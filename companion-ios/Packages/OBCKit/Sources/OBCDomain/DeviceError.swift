import Foundation

/// Typed transport/protocol failures the UI turns into design states (S3 error,
/// H7/H8 radio, H10 interrupted, protocol-mismatch banner). Kept an enum so
/// screens switch over it exhaustively.
///
/// **B-S0 skeleton** — the cases here are the contract-level failures named in
/// `OBCProtocol.md`. `B1` finalizes the full set (radio/permission states, etc.)
/// as it wires `BLETransport`.
public enum DeviceError: Error, Equatable, Sendable {
    /// Not connected to a device (link down / never bonded).
    case notConnected
    /// A control-plane read failed.
    case readFailed
    /// A control-plane write failed (e.g. `writeConfig`).
    case writeFailed
    /// A bulk transfer dropped mid-flight; resumable from `TransferProgress.offset`.
    case transferDropped
    /// A received object failed CRC validation before commit (see `OBCProtocol.md`
    /// → *CoC framing*). The object is rejected, never committed.
    case crcMismatch
    /// The device's `protocol_version` does not match `OBCProtocol.version`.
    /// Surfaced, not fatal.
    case protocolMismatch(expected: UInt16, found: UInt16)
}
