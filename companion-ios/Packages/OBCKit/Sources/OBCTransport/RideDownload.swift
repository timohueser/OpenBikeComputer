import Foundation
import OBCDomain

/// One ride landed by a `downloadRides` batch: its compact-binary object exactly
/// as the device stores it (layout is firmware-`S0`-owned). The device ride codec
/// decodes `payload` into the canonical `Ride`; interchange files (GPX/FIT/…) are
/// then encoded from that via `OBCFormats` — never straight from these bytes.
public struct DownloadedRide: Equatable, Sendable {
    public let id: RideID
    public let payload: Data

    public init(id: RideID, payload: Data) {
        self.id = id
        self.payload = payload
    }
}

/// A running ride sync: the batch's `TransferHandle` (progress / cancel /
/// resume) plus the rides themselves as they land. Partial results are
/// first-class: each ride is yielded as soon as its bytes are complete and
/// CRC-verified, so a drop mid-batch keeps everything already yielded and
/// `handle.resume()` continues into both streams.
public struct RideDownload: Sendable {
    public let handle: TransferHandle
    /// One element per requested ride, in transfer order. Finishes when the batch
    /// completes or is canceled; throws on unrecoverable failure (`crcMismatch`).
    public let rides: AsyncThrowingStream<DownloadedRide, Error>

    public init(handle: TransferHandle, rides: AsyncThrowingStream<DownloadedRide, Error>) {
        self.handle = handle
        self.rides = rides
    }

    /// A degenerate download with both streams already finished — the "nothing to
    /// pull" cases: already up to date (`.completed`, the default) or not
    /// connected (pass `.failed(.notConnected)`).
    public static func finished(_ outcome: TransferOutcome = .completed) -> RideDownload {
        let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        continuation.finish()
        return RideDownload(handle: .immediatelyFinished(outcome), rides: stream)
    }
}
