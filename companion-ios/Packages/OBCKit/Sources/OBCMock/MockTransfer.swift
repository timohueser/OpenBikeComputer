#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// One simulated bulk transfer. Instead of streaming real bytes over a channel, it
/// emits `TransferProgress` ticks paced by `throughputBytesPerSec`, so a progress bar
/// moves realistically with **no wire protocol** (the mock's realism comes from
/// timing + faults, per the issue). An actor so `cancel()` / `resume()` and the pump
/// loop don't race — mirroring `BLEChannel`'s `Uploader`.
///
/// Offset-resume matches the real path: a **drop** stops with the stream *open* at the
/// last committed offset (so `resume()` continues into it); a **cancel** finishes the
/// stream. A drop also toggles the link `.outOfRange` (the realistic, observable cause
/// behind H10) and `resume()` restores `.connected`.
actor MockTransfer {
    private let total: Int
    private let throughput: Int
    private let dropOffset: Int?
    private let linkChange: @Sendable (ConnectionState) -> Void
    private let progress: AsyncStream<TransferProgress>.Continuation

    private var committed = 0
    private var running = false
    private var canceled = false
    private var finished = false
    private var didDrop = false

    /// ~100 progress ticks across the object → a smooth bar without a flood of updates.
    private var stepBytes: Int { max(1, total / 100) }

    init(
        total: Int,
        throughputBytesPerSec: Int,
        dropAtFraction: Double?,
        linkChange: @escaping @Sendable (ConnectionState) -> Void,
        progress: AsyncStream<TransferProgress>.Continuation
    ) {
        self.total = max(0, total)
        self.throughput = max(1, throughputBytesPerSec)
        self.dropOffset = dropAtFraction.map { Int(Double(max(0, total)) * min(max(0, $0), 1)) }
        self.linkChange = linkChange
        self.progress = progress
    }

    func start() async { await pump() }

    private func pump() async {
        guard !running, !canceled, !finished else { return }
        running = true

        while committed < total {
            if canceled { break }

            // Armed drop point: stop with the stream open so resume() can continue.
            if let dropOffset, !didDrop, committed >= dropOffset {
                didDrop = true
                running = false
                linkChange(.outOfRange)
                return
            }

            let step = min(stepBytes, total - committed)
            let seconds = Double(step) / Double(throughput)
            try? await Task.sleep(for: .nanoseconds(Int64(seconds * 1_000_000_000)))
            if canceled { break }

            committed += step
            progress.yield(TransferProgress(bytesDone: committed, total: total, offset: committed))
        }

        running = false
        if canceled || committed >= total { complete() }
    }

    func cancel() {
        canceled = true
        if !running { complete() }  // if the pump already parked (post-drop), finish now
    }

    func resume() async {
        guard !canceled, !finished, committed < total else { return }
        // `didDrop` stays set so the pump won't re-trigger the (one-time) drop point;
        // just restore the link and continue from the last committed offset.
        if didDrop { linkChange(.connected) }
        await pump()
    }

    private func complete() {
        guard !finished else { return }
        finished = true
        progress.finish()
    }
}
#endif
