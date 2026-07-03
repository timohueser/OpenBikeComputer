import Foundation

/// Progress of a running bulk transfer over the L2CAP CoC data plane.
///
/// There is deliberately no resume offset: interrupted transfers **restart, not
/// resume**, so after a drop the counter simply starts over from 0.
public struct TransferProgress: Equatable, Sendable {
    /// Bytes transferred so far.
    public let bytesDone: Int
    /// Total object size in bytes (the descriptor's `total_len`).
    public let total: Int

    public init(bytesDone: Int, total: Int) {
        self.bytesDone = bytesDone
        self.total = total
    }

    /// Completed fraction in `0...1` (0 when `total` is unknown/zero).
    public var fraction: Double {
        total > 0 ? Double(bytesDone) / Double(total) : 0
    }
}

/// How a bulk transfer ended. A **drop is not terminal**: a dropped transfer
/// stays unresolved (link `.outOfRange`) until it is retried to completion,
/// canceled, or fails for good.
public enum TransferOutcome: Equatable, Sendable {
    case completed
    case canceled
    case failed(DeviceError)
}
