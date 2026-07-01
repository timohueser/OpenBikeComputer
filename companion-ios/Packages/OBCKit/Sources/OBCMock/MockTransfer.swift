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
    /// One ride inside a download batch — `byteCount` bytes of this batch belong to
    /// ride `id`. When the pump's committed count crosses a segment's end, the ride
    /// is "landed" and its synthesized payload is yielded into `rides`.
    struct Segment: Sendable {
        let id: RideID
        let byteCount: Int
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
            yieldLandedRides()
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

    /// Yield every ride whose bytes are now fully committed. Ride payloads are
    /// synthesized on the spot (the mock's realism is timing + faults, not bytes).
    /// A drop stops *between* rides landing, so partial batches match H10 exactly:
    /// what was yielded stays, resume lands the rest.
    private func yieldLandedRides() {
        var boundary = segments.prefix(nextSegment).reduce(0) { $0 + $1.byteCount }
        while nextSegment < segments.count {
            let segment = segments[nextSegment]
            boundary += segment.byteCount
            guard boundary <= committed else { break }
            rides?.yield(DownloadedRide(id: segment.id, payload: MockPayload.make(count: segment.byteCount)))
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
