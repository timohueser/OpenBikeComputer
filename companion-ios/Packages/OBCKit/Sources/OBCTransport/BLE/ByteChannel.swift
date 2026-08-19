import Foundation

/// The byte-stream seam under `BLEChannel` — an ordered, reliable duplex pipe.
/// The **real** conformer is `L2CAPByteChannel` (a CoreBluetooth `CBL2CAPChannel`);
/// the tests use an in-memory pipe. This indirection keeps physical byte movement host-testable.
///
/// Reads return **however many bytes are available** (like a socket) — `BLEChannel`
/// reassembles frames from arbitrary read sizes. A clean end-of-stream returns an
/// empty `Data`.
public protocol ByteChannel: Sendable {
    /// Write all of `data`, or throw if the channel dropped.
    func write(_ data: Data) async throws
    /// Read up to `maxLength` bytes. Returns empty `Data` at end of stream.
    func read(maxLength: Int) async throws -> Data
    /// Cancel a parked read while keeping the physical channel open.
    func cancelRead()
    /// Tear the channel down (idempotent).
    func close() async
}

public extension ByteChannel {
    func cancelRead() {}
}

/// Thrown by a `ByteChannel` when the underlying link drops mid-transfer. The
/// protocol client catches this and reconciles durable state before restarting at offset zero.
public struct ChannelDropped: Error, Sendable {
    public init() {}
}
