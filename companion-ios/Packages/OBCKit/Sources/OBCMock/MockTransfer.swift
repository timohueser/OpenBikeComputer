#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// One simulated bulk transfer. It emits `TransferProgress` ticks paced by
/// `throughputBytesPerSec` instead of streaming bytes, so a progress bar moves realistically
/// with no wire protocol. An actor, so `cancel()` and `resume()` cannot race the pump loop.
///
/// Restart semantics match the real path: a drop stops with the stream open and toggles the
/// link to `.outOfRange`; `resume()` restores the link and starts over, from byte 0 for a
/// single upload, or from the last fully-landed ride of a download batch. A cancel finishes
/// the stream terminally.
actor MockTransfer {
    /// One ride inside a download batch: `byteCount` bytes of the batch belong to ride `id`
    /// (pacing only). When the committed count crosses a segment's end, its `payload` is yielded.
    struct Segment: Sendable {
        let id: RideID
        let byteCount: Int
        let payload: Data
    }

    private let total: Int
    private let throughput: Int
    private let dropOffset: Int?
    private let segments: [Segment]
    private let rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation?
    private let linkChange: @Sendable (ConnectionState) -> Void
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>

    private var committed = 0
    private var nextSegment = 0
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
        segments: [Segment] = [],
        rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation? = nil,
        linkChange: @escaping @Sendable (ConnectionState) -> Void,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>
    ) {
        self.total = max(0, total)
        self.throughput = max(1, throughputBytesPerSec)
        self.dropOffset = dropAtFraction.map { Int(Double(max(0, total)) * min(max(0, $0), 1)) }
        self.segments = segments
        self.rides = rides
        self.linkChange = linkChange
        self.progress = progress
        self.outcome = outcome
    }

    func start() async { await pump() }

    private func pump() async {
        guard !running, !canceled, !finished else { return }
        running = true

        while committed < total {
            if canceled { break }

            // Armed drop point: stop with the stream open so resume() can restart.
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
            progress.yield(TransferProgress(bytesDone: committed, total: total))
            yieldLandedRides()
        }

        running = false
        if canceled || committed >= total { complete() }
    }

    func cancel() {
        canceled = true
        if !running { complete() }  // if the pump already parked (post-drop), finish now
    }

    /// Restore the link and restart: rides that fully landed stay landed, and everything past
    /// the last segment boundary is re-sent. `didDrop` stays set, so the drop point fires once.
    func resume() async {
        guard !canceled, !finished, committed < total else { return }
        committed = segments.prefix(nextSegment).reduce(0) { $0 + $1.byteCount }
        if didDrop { linkChange(.connected) }
        await pump()
    }

    /// Yield every ride whose bytes are now fully committed. A drop stops between rides landing,
    /// so what was yielded stays and a restart lands the rest.
    private func yieldLandedRides() {
        var boundary = segments.prefix(nextSegment).reduce(0) { $0 + $1.byteCount }
        while nextSegment < segments.count {
            let segment = segments[nextSegment]
            boundary += segment.byteCount
            guard boundary <= committed else { break }
            rides?.yield(DownloadedRide(id: segment.id, payload: segment.payload))
            nextSegment += 1
        }
    }

    private func complete() {
        guard !finished else { return }
        finished = true
        progress.finish()
        rides?.finish()
        outcome.fulfill(canceled ? .canceled : .completed)
    }
}
#endif
