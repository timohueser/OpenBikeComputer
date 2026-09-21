import Foundation

/// Typed transport and protocol failures the UI turns into design states. An enum, so screens
/// switch over it exhaustively.
public enum DeviceError: Error, Equatable, Sendable {
    /// Mirrors the actionable subset of `CBManagerState`.
    public enum BluetoothUnavailableReason: Equatable, Sendable {
        case poweredOff
        case unauthorized
        case unsupported
    }

    case notConnected
    case bluetoothUnavailable(BluetoothUnavailableReason)
    case deviceNotFound
    /// The L2CAP CoC channel could not be opened: the PSM read or the open failed.
    case channelOpenFailed
    /// LESC pairing did not complete: the passkey was declined or wrong, or the encrypted link
    /// the gated characteristics require was refused.
    case pairingFailed
    case readFailed
    case writeFailed
    /// A bulk transfer dropped mid-flight. A transfer restarts whole; it does not resume.
    case transferDropped
    /// The device answered a transfer with a terminal reject and committed nothing. An unknown
    /// transfer status code also lands here, so a reject the app cannot name never traps.
    case transferRejected
    /// The device rejected a new-route upload because its route storage is full. The reject
    /// lands at descriptor-open time, before the device consumes payload, so nothing is
    /// committed. A replace-by-id upload reuses a slot and never sees this.
    case storageFull
    /// A received object failed CRC validation, so it was rejected before commit.
    case crcMismatch
    /// The device's `protocol_version` does not match `OBCProtocol.version`.
    /// Surfaced, not fatal.
    case protocolMismatch(expected: UInt16, found: UInt16)
}
