import Foundation

/// Typed transport/protocol failures the UI turns into design states. Kept an
/// enum so screens switch over it exhaustively.
public enum DeviceError: Error, Equatable, Sendable {
    /// Why the Bluetooth radio can't be used. Mirrors the actionable subset of
    /// `CBManagerState`.
    public enum BluetoothUnavailableReason: Equatable, Sendable {
        case poweredOff
        case unauthorized
        case unsupported
    }

    /// Not connected to a device (link down / never bonded).
    case notConnected
    /// The Bluetooth radio is off, denied, or unsupported.
    case bluetoothUnavailable(BluetoothUnavailableReason)
    /// Scanning finished without finding an OBC device to connect to.
    case deviceNotFound
    /// The L2CAP CoC channel could not be opened (PSM read or open failed).
    case channelOpenFailed
    /// LESC pairing didn't complete — the passkey was declined/wrong, or the
    /// encrypted link the gated characteristics require was refused.
    case pairingFailed
    /// A control-plane read failed.
    case readFailed
    /// A control-plane write failed (e.g. `writeConfig`).
    case writeFailed
    /// A bulk transfer dropped mid-flight; the whole object is re-sent /
    /// re-requested (transfers restart, not resume).
    case transferDropped
    /// The device answered a transfer with a terminal reject (`error` /
    /// `notFound` / `busy`, spec §4.3) — nothing was committed.
    case transferRejected
    /// A received object failed CRC validation before commit (see `OBCProtocol.md`
    /// → *CoC framing*). The object is rejected, never committed.
    case crcMismatch
    /// The device's `protocol_version` does not match `OBCProtocol.version`.
    /// Surfaced, not fatal.
    case protocolMismatch(expected: UInt16, found: UInt16)
}
