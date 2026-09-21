import Foundation
import Observation

/// The app's in-flight transfer ledger: what the foreground-only link policy consults
/// before an intentional background disconnect. An in-flight transfer or ride-sync
/// batch is never dropped; it drains under the system grace window first.
///
/// It is app-level, not transport-level: `BLETransport`'s transfer slot is per-object,
/// so a ride-sync batch is free-slotted between rides and only the models know a batch
/// is mid-flight. Tokens make `end` idempotent per claim, because models end on several
/// exit paths and must not double-release.
///
/// `@Observable` so the composition root can drive the idle timer off `isActive`
/// without a poll.
@MainActor @Observable
public final class TransferActivity {
    /// One in-flight job's claim; identity only.
    public final class Token {
        public init() {}
    }

    private var open: Set<ObjectIdentifier> = []
    @ObservationIgnored private var waiters: [UUID: CheckedContinuation<Void, Never>] = [:]

    public init() {}

    /// Whether any job currently holds a claim.
    public var isActive: Bool { !open.isEmpty }

    public func begin() -> Token {
        let token = Token()
        open.insert(ObjectIdentifier(token))
        return token
    }

    /// Release a claim (idempotent per token). The last release resumes every
    /// `waitUntilIdle()` waiter.
    public func end(_ token: Token) {
        open.remove(ObjectIdentifier(token))
        guard open.isEmpty, !waiters.isEmpty else { return }
        let parked = waiters
        waiters.removeAll()
        for continuation in parked.values { continuation.resume() }
    }

    /// Suspend until the ledger is empty; returns at once when it already is.
    /// A canceled waiter resumes immediately: grace-window expiry must not leak a
    /// parked continuation.
    public func waitUntilIdle() async {
        guard isActive else { return }
        let id = UUID()
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                // A cancel can land before this registration, because the handler's
                // main-actor hop is serialized behind us. Park only while the task is
                // live, or the waiter never resumes.
                if !isActive || Task.isCancelled {
                    continuation.resume()
                } else {
                    waiters[id] = continuation
                }
            }
        } onCancel: {
            Task { @MainActor in
                self.waiters.removeValue(forKey: id)?.resume()
            }
        }
    }
}
