import Foundation

/// Progress of a running bulk transfer over the L2CAP CoC data plane — what the
/// upload sheet (B5) and ride sync (B7) render. `offset` is the resume anchor:
/// the byte position a dropped transfer restarts from (see `OBCProtocol.md` →
/// *CoC framing*, `offset`).
///
/// **B-S0 skeleton** — `B1` produces these from the framing layer and exposes
/// them via `TransferHandle.progress`.
public struct TransferProgress: Equatable, Sendable {
    /// Bytes transferred and committed so far.
    public let bytesDone: Int
    /// Total object size in bytes (`total_len` in the frame header).
    public let total: Int
    /// Byte offset the transfer would resume from if interrupted.
    public let offset: Int

    public init(bytesDone: Int, total: Int, offset: Int) {
        self.bytesDone = bytesDone
        self.total = total
        self.offset = offset
    }

    /// Completed fraction in `0...1` (0 when `total` is unknown/zero).
    public var fraction: Double {
        total > 0 ? Double(bytesDone) / Double(total) : 0
    }
}

/// How a bulk transfer ended — the terminal state `TransferHandle.outcome`
/// resolves to, so the upload sheet (B5/F) and ride sync (B7) never have to infer
/// completion from byte counts. A **drop is not terminal**: a dropped transfer
/// stays unresolved (stalled at its resume offset, link `.outOfRange`) until it
/// resumes to completion, is canceled, or fails for good.
public enum TransferOutcome: Equatable, Sendable {
    case completed
    case canceled
    case failed(DeviceError)
}
