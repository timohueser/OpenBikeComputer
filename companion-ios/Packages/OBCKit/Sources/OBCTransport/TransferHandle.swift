import Foundation
import OBCDomain

/// A running bulk transfer the UI observes and controls — the return of
/// `uploadRoute` (B5) and `downloadRides` (B7). A `Sendable` value type backed by
/// the runner driving the transfer: `progress` streams `TransferProgress`, and
/// `cancel()` / `resume()` signal that runner.
///
/// `resume()` **restarts, it does not continue** (spec §1 principle 4): a dropped
/// upload is re-sent whole (progress starts over from 0); a dropped ride batch
/// re-requests only the rides that haven't fully landed — whole objects are the
/// resume granularity.
public struct TransferHandle: Sendable {
    /// Progress updates as the transfer advances. Finishes when the transfer
    /// completes or is canceled. A **drop** does *not* finish it — the stream
    /// stalls open so a restart can continue into it (the observable drop signal
    /// is `DeviceTransport.state` → `.outOfRange`).
    public let progress: AsyncStream<TransferProgress>

    private let outcomePromise: AsyncPromise<TransferOutcome>
    private let assignedObjectIDPromise: AsyncPromise<DeviceObjectID?>?
    private let onCancel: @Sendable () -> Void
    private let onResume: @Sendable () -> Void

    public init(
        progress: AsyncStream<TransferProgress>,
        outcome: AsyncPromise<TransferOutcome>,
        assignedObjectID: AsyncPromise<DeviceObjectID?>? = nil,
        onCancel: @escaping @Sendable () -> Void,
        onResume: @escaping @Sendable () -> Void
    ) {
        self.progress = progress
        self.outcomePromise = outcome
        self.assignedObjectIDPromise = assignedObjectID
        self.onCancel = onCancel
        self.onResume = onResume
    }

    /// The terminal state — resolves when the transfer completes, is canceled, or
    /// fails for good, so the UI never infers success from byte counts. A drop
    /// keeps it unresolved (the transfer is still restartable); pair with
    /// `progress` + `DeviceTransport.state` for the interrupted (F/H10)
    /// presentation.
    public var outcome: TransferOutcome {
        get async { await outcomePromise.value }
    }

    /// The terminal state if already reached, `nil` while the transfer is live or
    /// dropped-but-restartable (never suspends).
    public var currentOutcome: TransferOutcome? { outcomePromise.current }

    /// The device-assigned object id, for a **route upload** — resolves after the
    /// transfer commits (the device reports it in the `transferResult`). `nil` when
    /// this handle carries no id (a download, an immediately-finished handle, or a
    /// pre-bring-up BLE path). Await *after* `outcome == .completed`.
    public var assignedObjectID: DeviceObjectID? {
        get async { assignedObjectIDPromise == nil ? nil : await assignedObjectIDPromise!.value }
    }

    /// Abort the transfer and tear the channel down cleanly.
    public func cancel() { onCancel() }

    /// Restart a dropped transfer (whole upload / the batch's missing rides).
    public func resume() { onResume() }

    /// A degenerate handle: progress already finished, controls no-ops, and
    /// `outcome` pre-resolved — `.completed` for "nothing to do" (H9 up to date),
    /// `.failed(.notConnected)` for "no transfer possible" (H4).
    public static func immediatelyFinished(_ outcome: TransferOutcome = .completed) -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        continuation.finish()
        let promise = AsyncPromise<TransferOutcome>()
        promise.fulfill(outcome)
        return TransferHandle(progress: stream, outcome: promise, onCancel: {}, onResume: {})
    }
}
