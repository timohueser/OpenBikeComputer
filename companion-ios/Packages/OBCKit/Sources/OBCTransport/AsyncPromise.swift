import Foundation

/// A one-shot async value: fulfilled exactly once and awaitable by any number of readers,
/// where a late reader gets the value immediately. The first `fulfill` wins and later calls do
/// nothing, so racing terminal paths resolve to whichever landed first. It backs
/// `TransferHandle.outcome`.
public final class AsyncPromise<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var resolved: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    public init() {}

    public func fulfill(_ value: Value) {
        lock.lock()
        guard resolved == nil else { lock.unlock(); return }
        resolved = value
        let waiting = waiters
        waiters = []
        lock.unlock()
        for waiter in waiting { waiter.resume(returning: value) }
    }

    /// The resolved value. It suspends until `fulfill` when it is not yet resolved.
    public var value: Value {
        get async {
            await withCheckedContinuation { continuation in
                lock.lock()
                if let resolved {
                    lock.unlock()
                    continuation.resume(returning: resolved)
                } else {
                    waiters.append(continuation)
                    lock.unlock()
                }
            }
        }
    }

    /// The value if it is already fulfilled, `nil` otherwise. It never suspends.
    public var current: Value? {
        lock.lock(); defer { lock.unlock() }
        return resolved
    }
}
