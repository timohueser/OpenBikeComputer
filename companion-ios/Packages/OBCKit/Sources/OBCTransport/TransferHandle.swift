import Foundation
import OBCDomain

/// A running bulk transfer the UI observes and controls: the return of `uploadRoute` and
/// `downloadRides`.
///
/// `resume()` restarts, it does not continue: a dropped upload is re-sent whole and progress
/// starts again at 0. A dropped ride batch re-requests only the rides that did not fully land.
public struct TransferHandle: Sendable {
    /// Progress updates as the transfer advances. It finishes when the transfer completes or is
    /// canceled. A drop does not finish it: the stream stays open so a restart continues into it.
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

    /// The terminal state. It resolves when the transfer completes, is canceled, or fails for
    /// good, so the UI never infers success from byte counts. A drop keeps it unresolved.
    public var outcome: TransferOutcome {
        get async { await outcomePromise.value }
    }

    /// The terminal state if it is already reached, `nil` otherwise. It never suspends.
    public var currentOutcome: TransferOutcome? { outcomePromise.current }

    /// The device-assigned object id for a route upload. It resolves after the transfer commits.
    /// `nil` when this handle carries no id, such as a download. Await it after `outcome`.
    public var assignedObjectID: DeviceObjectID? {
        get async { assignedObjectIDPromise == nil ? nil : await assignedObjectIDPromise!.value }
    }

    /// Abort the transfer and tear the channel down cleanly.
    public func cancel() { onCancel() }

    public func resume() { onResume() }

    /// A degenerate handle: progress is already finished, the controls do nothing, and
    /// `outcome` is pre-resolved.
    public static func immediatelyFinished(_ outcome: TransferOutcome = .completed) -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        continuation.finish()
        let promise = AsyncPromise<TransferOutcome>()
        promise.fulfill(outcome)
        return TransferHandle(progress: stream, outcome: promise, onCancel: {}, onResume: {})
    }
}
