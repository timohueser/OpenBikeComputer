import Foundation

/// A last-value multicast for `AsyncStream`: each `stream()` replays the most recent element
/// and then receives live updates, and `send` fans out to every live subscriber. `BLETransport`
/// backs its `state` and `battery` streams with it.
public final class AsyncMulticast<Element: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var last: Element
    private var continuations: [UUID: AsyncStream<Element>.Continuation] = [:]
    private var finished = false

    public init(_ initial: Element) {
        self.last = initial
    }

    public var value: Element {
        lock.lock(); defer { lock.unlock() }
        return last
    }

    public func stream() -> AsyncStream<Element> {
        AsyncStream { continuation in
            lock.lock()
            let done = finished
            let id = UUID()
            if !done { continuations[id] = continuation }
            // Registration and replay are atomic against `send`: the replay is yielded under
            // the lock, so a concurrent `send` cannot slip its newer value in before the older
            // replay. This is safe because `yield` only buffers; it never reenters consumer code.
            continuation.yield(last)
            lock.unlock()

            if done { continuation.finish(); return }

            continuation.onTermination = { [weak self] _ in
                guard let self else { return }
                self.lock.lock(); self.continuations[id] = nil; self.lock.unlock()
            }
        }
    }

    public func send(_ value: Element) {
        lock.lock()
        last = value
        let targets = Array(continuations.values)
        lock.unlock()
        for continuation in targets { continuation.yield(value) }
    }

    /// Finish every subscriber's stream; later subscribers get one replayed value
    /// then a finished stream.
    public func finish() {
        lock.lock()
        finished = true
        let targets = Array(continuations.values)
        continuations.removeAll()
        lock.unlock()
        for continuation in targets { continuation.finish() }
    }
}
