import Foundation
import Observation

/// The app's in-flight transfer ledger (#459) — what the foreground-only link
/// policy consults before an intentional background disconnect: **an in-flight
/// transfer or ride-sync batch is never dropped**, it drains under the system
/// grace window first.
///
/// Deliberately app-level, not transport-level: `BLETransport`'s transfer slot
/// is per-object, so a ride-sync *batch* is free-slotted between rides — only
/// the models know a batch is mid-flight. `UploadSheetModel` claims a token
/// while an upload runs; `RideSyncCoordinator` while its batch is `.syncing`
/// (a stalled interruption releases it — waiting longer won't finish a
/// transfer whose device is gone).
///
/// Tokens make `end` idempotent per claim: models end on several exit paths
/// (terminal outcome, drop-watch, sheet teardown) and must not double-release.
///
/// `@Observable` so the composition root can drive UIKit off `isActive` without
/// a poll — the #754 idle-timer guard disables `isIdleTimerDisabled` exactly
/// while the ledger holds a claim. Observation stays out of OBCKit's own logic;
/// only `open` (what `isActive` reads) is tracked, the continuation bookkeeping
/// is `@ObservationIgnored`.
@MainActor @Observable
public final class TransferActivity {
    /// One in-flight job's claim — identity only.
    public final class Token {
        public init() {}
    }

    private var open: Set<ObjectIdentifier> = []
    @ObservationIgnored private var waiters: [UUID: CheckedContinuation<Void, Never>] = [:]

    public init() {}

    /// Whether any transfer/sync currently holds a claim.
    public var isActive: Bool { !open.isEmpty }

    /// Claim a slot in the ledger for one in-flight job.
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

    /// Suspend until the ledger is empty (returns at once when it already is).
    /// Cancellation-responsive: a canceled waiter resumes immediately — the
    /// grace-window expiry must not leak a parked continuation.
    public func waitUntilIdle() async {
        guard isActive else { return }
        let id = UUID()
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                // A cancel can land before this registration (the handler's
                // main-actor hop is serialized behind us) — park only when the
                // task is still live, or the waiter would never resume.
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
