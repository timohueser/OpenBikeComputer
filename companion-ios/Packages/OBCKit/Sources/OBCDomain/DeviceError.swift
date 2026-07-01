import Foundation

/// Typed transport/protocol failures the UI turns into design states (S3 error,
/// H7/H8 radio, H10 interrupted, protocol-mismatch banner). Kept an enum so
/// screens switch over it exhaustively.
///
/// **B1 finalization:** adds the radio/permission (`bluetoothUnavailable`),
/// discovery (`deviceNotFound`), and CoC-open (`channelOpenFailed`) cases that
/// `BLETransport` actually throws, alongside the contract-level failures named in
/// `OBCProtocol.md`.
public enum DeviceError: Error, Equatable, Sendable {
    /// Why the Bluetooth radio can't be used — drives the H7/H8 permission &
    /// power states. Mirrors the actionable subset of `CBManagerState`.
    public enum BluetoothUnavailableReason: Equatable, Sendable {
        case poweredOff
        case unauthorized
        case unsupported
    }

    /// Not connected to a device (link down / never bonded).
    case notConnected
    /// The Bluetooth radio is off, denied, or unsupported (H7/H8).
    case bluetoothUnavailable(BluetoothUnavailableReason)
    /// Scanning finished without finding an OBC device to connect to.
    case deviceNotFound
    /// The L2CAP CoC channel could not be opened (PSM read or open failed).
    case channelOpenFailed
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
