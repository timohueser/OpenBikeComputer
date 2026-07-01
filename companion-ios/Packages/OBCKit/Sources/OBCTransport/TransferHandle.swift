import Foundation
import OBCDomain

/// A running bulk transfer the UI observes and controls — the return of
/// `uploadRoute` (B5) and `downloadRides` (B7). A `Sendable` value type backed by
/// the actor running the transfer: `progress` streams `TransferProgress`, and
/// `cancel()` / `resume()` signal that actor.
///
/// `resume()` is **offset-based**: after a drop, the transfer restarts from the
/// last committed `TransferProgress.offset` (see `OBCProtocol.md` → *CoC framing*).
public struct TransferHandle: Sendable {
    /// Progress updates as the transfer advances. Finishes when the transfer
    /// completes, is canceled, or drops.
    public let progress: AsyncStream<TransferProgress>

    private let onCancel: @Sendable () -> Void
    private let onResume: @Sendable () -> Void

    public init(
        progress: AsyncStream<TransferProgress>,
        onCancel: @escaping @Sendable () -> Void,
        onResume: @escaping @Sendable () -> Void
    ) {
        self.progress = progress
        self.onCancel = onCancel
        self.onResume = onResume
    }

    /// Abort the transfer and tear the channel down cleanly.
    public func cancel() { onCancel() }

    /// Resume a dropped transfer from its last committed offset.
    public func resume() { onResume() }

    /// A degenerate handle whose progress stream is already finished and whose
    /// controls are no-ops — for the "no transfer possible" case (not connected,
    /// or a mock stub). Callers detect the real state via `DeviceTransport.state`.
    public static func immediatelyFinished() -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        continuation.finish()
        return TransferHandle(progress: stream, onCancel: {}, onResume: {})
    }
}
