import Foundation

/// A last-value multicast for `AsyncStream`: replay the latest value for
/// `state`/`battery` so a late subscriber gets the current value immediately.
/// Each `stream()` replays the most recent element, then receives live updates;
/// `send` fans out to all live subscribers.
///
/// Pure (no CoreBluetooth), so it's unit-tested directly. `BLETransport` uses it
/// to back its `state` and `battery` streams.
public final class AsyncMulticast<Element: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var last: Element
    private var continuations: [UUID: AsyncStream<Element>.Continuation] = [:]
    private var finished = false

    public init(_ initial: Element) {
        self.last = initial
    }

    /// The current latest value.
    public var value: Element {
        lock.lock(); defer { lock.unlock() }
        return last
    }

    /// A new subscription: replays the latest value, then streams live updates.
    public func stream() -> AsyncStream<Element> {
        AsyncStream { continuation in
            lock.lock()
            let replay = last
            let done = finished
            let id = UUID()
            if !done { continuations[id] = continuation }
            lock.unlock()

            continuation.yield(replay)
            if done { continuation.finish(); return }

            continuation.onTermination = { [weak self] _ in
                guard let self else { return }
                self.lock.lock(); self.continuations[id] = nil; self.lock.unlock()
            }
        }
    }

    /// Update the latest value and fan out to all live subscribers.
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
