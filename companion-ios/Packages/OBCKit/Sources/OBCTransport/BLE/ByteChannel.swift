import Foundation

/// The byte-stream seam under `BLEChannel` — an ordered, reliable duplex pipe.
/// The **real** conformer is `L2CAPByteChannel` (a CoreBluetooth `CBL2CAPChannel`);
/// the tests use an in-memory pipe. This indirection is what lets the entire
/// framing/resume/cancel layer be exercised under `swift test` with no hardware.
///
/// Reads return **however many bytes are available** (like a socket) — `BLEChannel`
/// reassembles frames from arbitrary read sizes. A clean end-of-stream returns an
/// empty `Data`.
public protocol ByteChannel: Sendable {
    /// Write all of `data`, or throw if the channel dropped.
    func write(_ data: Data) async throws
    /// Read up to `maxLength` bytes. Returns empty `Data` at end of stream.
    func read(maxLength: Int) async throws -> Data
    /// Tear the channel down (idempotent).
    func close() async
}

/// Thrown by a `ByteChannel` when the underlying link drops mid-transfer. The
/// transfer layer catches this and leaves the upload resumable from its last
/// committed offset.
public struct ChannelDropped: Error, Sendable {
    public init() {}
}
