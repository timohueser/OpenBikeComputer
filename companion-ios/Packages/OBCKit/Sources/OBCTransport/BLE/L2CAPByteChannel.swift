#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation

/// A `ByteChannel` backed by a CoreBluetooth `CBL2CAPChannel` — the real CoC data
/// plane. The channel's `InputStream`/`OutputStream` are scheduled on a dedicated
/// run-loop thread; `read`/`write` bridge the stream delegate callbacks to
/// async/await via continuations.
///
/// **Every stream touch happens on that thread.** `NSStream` is not thread-safe:
/// pumping the output stream from the async caller's thread (as this class first
/// did) races the delegate events and can silently miss a `hasSpaceAvailable`
/// re-arm — the transfer then sits wedged forever with the link nominally up. So
/// `write` only enqueues and hops the pump over, and a **stall watchdog** on the
/// same run loop backstops whatever slips through anyway: a parked read/write
/// that moves no bytes for [`stallTimeout`] fails the channel (`ChannelDropped`),
/// which the layers above already treat as a drop — the device discards its
/// partial when the CoC closes, and the upload sheet offers Resume instead of
/// hanging at N%.
///
/// `public` so host tooling can wrap a `CBL2CAPChannel` in the same byte layer as `BLETransport`.
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

    /// How long a parked read/write may sit with **zero byte movement** before the
    /// channel is declared dead. Generous: even the slowest negotiated link moves a
    /// chunk every connection interval, and the device's longest quiet stretch (the
    /// pre-announce CRC pass) is well under a second per megabyte.
    private let stallTimeout: TimeInterval

    private var inbound = Data()
    private var outbound = Data()
    private var readWaiter: (max: Int, cont: CheckedContinuation<Data, Error>)?
    private var writeWaiter: CheckedContinuation<Void, Error>?
    private var closed = false
    private var failed = false
    /// When bytes last moved (or a waiter parked) — the watchdog's reference point.
    private var lastActivity = Date()
    private var stallTimer: Timer?

    public init(channel: CBL2CAPChannel, stallTimeout: TimeInterval = 10) {
        self.channel = channel
        self.input = channel.inputStream
        self.output = channel.outputStream
        self.stallTimeout = stallTimeout
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
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            self?.checkStall()
        }
        RunLoop.current.add(timer, forMode: .default)
        stallTimer = timer
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
            lastActivity = Date()
            lock.unlock()
            // The pump touches the stream, so it runs where the stream lives.
            perform(#selector(pumpOutbound), on: thread, with: nil, waitUntilDone: false)
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
            lastActivity = Date()
            lock.unlock()
        }
    }

    public func close() async {
        // Lock + thread-hop live in a synchronous helper (both are flagged inside
        // an async body); resuming the waiter afterward is async-safe.
        beginClose()?.resume(returning: Data())
    }

    /// Cancel the one physical read waiter without closing the CoC. Protocol v4 uses this after
    /// the GET result arrives behind the final stream record.
    public func cancelRead() {
        lock.lock()
        let read = readWaiter
        readWaiter = nil
        lock.unlock()
        read?.cont.resume(throwing: CancellationError())
    }

    private func beginClose() -> CheckedContinuation<Data, Error>? {
        lock.lock()
        guard !closed else { lock.unlock(); return nil }
        closed = true
        let read = readWaiter; readWaiter = nil
        let write = writeWaiter; writeWaiter = nil
        lock.unlock()
        perform(#selector(teardown), on: thread, with: nil, waitUntilDone: false)
        // A parked writer (a backpressured `send` when the cancel/close lands) is
        // never re-armed by a stream event on a self-initiated close, so resume it
        // here or its continuation leaks (the awaiting `send` hangs forever). A
        // write fails like any other drop; the read resolves to a clean EOF,
        // returned to the async `close()`. `fail()` nils the same waiters under the
        // lock, so at most one of the two paths resumes each.
        write?.resume(throwing: ChannelDropped())
        return read?.cont
    }

    @objc private func teardown() {
        stallTimer?.invalidate()
        stallTimer = nil
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
            fail(cleanEnd: eventCode == .endEncountered)
        default:
            break
        }
    }

    private func drainInbound() {
        var scratch = [UInt8](repeating: 0, count: 4096)
        lock.lock()
        while input.hasBytesAvailable {
            let n = input.read(&scratch, maxLength: scratch.count)
            if n > 0 {
                inbound.append(contentsOf: scratch[0..<n])
                lastActivity = Date()
            } else {
                break
            }
        }
        guard let waiter = readWaiter, !inbound.isEmpty else { lock.unlock(); return }
        readWaiter = nil
        let count = Swift.min(waiter.max, inbound.count)
        let out = Data(inbound.prefix(count))
        inbound.removeFirst(count)
        lock.unlock()
        waiter.cont.resume(returning: out)
    }

    @objc private func pumpOutbound() {
        lock.lock()
        while output.hasSpaceAvailable, !outbound.isEmpty {
            let n = outbound.withUnsafeBytes { output.write($0.bindMemory(to: UInt8.self).baseAddress!, maxLength: outbound.count) }
            if n > 0 {
                outbound.removeFirst(n)
                lastActivity = Date()
            } else {
                break
            }
        }
        if outbound.isEmpty, let cont = writeWaiter {
            writeWaiter = nil
            lock.unlock()
            cont.resume()
            return
        }
        lock.unlock()
    }

    /// The watchdog tick (run-loop thread): a parked waiter with no byte movement
    /// for [`stallTimeout`] means the CoC is wedged — the link may still be "up",
    /// but this transfer will never finish. Fail the channel so the layers above
    /// recover (teardown → the device discards its partial → restart/Resume).
    private func checkStall() {
        lock.lock()
        let stalled = !closed && !failed
            && (readWaiter != nil || writeWaiter != nil)
            && Date().timeIntervalSince(lastActivity) > stallTimeout
        lock.unlock()
        if stalled { fail(cleanEnd: false) }
    }

    private func fail(cleanEnd: Bool) {
        lock.lock()
        guard !failed else { lock.unlock(); return }
        failed = true
        let read = readWaiter; readWaiter = nil
        let write = writeWaiter; writeWaiter = nil
        lock.unlock()
        if cleanEnd { read?.cont.resume(returning: Data()) } else { read?.cont.resume(throwing: ChannelDropped()) }
        write?.resume(throwing: ChannelDropped())
        // A failed channel never carries another transfer (`isOpen` is false; the
        // transport opens a fresh CoC instead) — release the streams and the
        // thread now, and let the `CBL2CAPChannel` close when this object dies.
        perform(#selector(teardown), on: thread, with: nil, waitUntilDone: false)
    }
}
#endif
