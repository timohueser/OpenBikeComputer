import Foundation

/// A one-shot async value: fulfilled exactly once, awaitable by any number of
/// readers before or after fulfillment (late readers get the value immediately).
/// The first `fulfill` wins; later calls are no-ops — so racing terminal paths
/// (complete vs cancel) resolve deterministically to whichever landed first.
///
/// Pure and lock-based like `AsyncMulticast` (its one-shot sibling). Backs
/// `TransferHandle.outcome`.
public final class AsyncPromise<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var resolved: Value?
    private var waiters: [CheckedContinuation<Value, Never>] = []

    public init() {}

    /// Resolve the promise. Idempotent — only the first value sticks.
    public func fulfill(_ value: Value) {
        lock.lock()
        guard resolved == nil else { lock.unlock(); return }
        resolved = value
        let waiting = waiters
        waiters = []
        lock.unlock()
        for waiter in waiting { waiter.resume(returning: value) }
    }

    /// The resolved value — suspends until `fulfill` if not yet resolved.
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

    /// The value if already fulfilled, `nil` otherwise (never suspends).
    public var current: Value? {
        lock.lock(); defer { lock.unlock() }
        return resolved
    }
}
