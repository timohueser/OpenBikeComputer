import Foundation
import OBCDomain

/// One ride landed by a `downloadRides` batch: the compact-binary object exactly as the device
/// stores it. The ride codec decodes `payload` into the canonical `Ride`, and interchange files
/// are encoded from that, never straight from these bytes.
public struct DownloadedRide: Equatable, Sendable {
    public let id: RideID
    public let payload: Data
    public let source: RideSource?

    public init(id: RideID, payload: Data, source: RideSource? = nil) {
        self.id = id
        self.payload = payload
        self.source = source
    }
}

/// A running ride sync: the batch's `TransferHandle` plus the rides as they land. Each ride is
/// yielded as soon as its bytes are complete and CRC-verified, so a drop mid-batch keeps every
/// ride already yielded and `handle.resume()` continues into both streams.
public struct RideDownload: Sendable {
    public let handle: TransferHandle
    /// One element per requested ride, in transfer order. It finishes when the batch completes
    /// or is canceled, and throws on unrecoverable failure.
    public let rides: AsyncThrowingStream<DownloadedRide, Error>

    public init(handle: TransferHandle, rides: AsyncThrowingStream<DownloadedRide, Error>) {
        self.handle = handle
        self.rides = rides
    }

    /// A degenerate download with both streams already finished: there is nothing to pull.
    public static func finished(_ outcome: TransferOutcome = .completed) -> RideDownload {
        let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        continuation.finish()
        return RideDownload(handle: .immediatelyFinished(outcome), rides: stream)
    }
}
