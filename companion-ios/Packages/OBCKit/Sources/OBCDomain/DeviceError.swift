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
    /// LESC pairing didn't complete — the passkey was declined/wrong, or the
    /// encrypted link the gated characteristics require was refused (firmware
    /// `A8`). Drives the D5 "pairing didn't finish" state.
    case pairingFailed
    /// A control-plane read failed.
    case readFailed
    /// A control-plane write failed (e.g. `writeConfig`).
    case writeFailed
    /// A bulk transfer dropped mid-flight; the whole object is re-sent /
    /// re-requested (transfers restart, not resume).
    case transferDropped
    /// The device answered a transfer with a terminal reject (`error` /
    /// `notFound` / `busy`, spec §4.3) — nothing was committed. An **unknown**
    /// transfer status code also lands here (forward compat: a reject the app
    /// doesn't recognize is still a generic device-side failure, never a trap).
    case transferRejected
    /// The device rejected a **new**-route upload because its route storage is
    /// full (`storageFull`, spec §4.3) — the reject lands at descriptor-open time,
    /// before any bytes stream, and nothing is committed. **Replace-by-id uploads
    /// of an existing route are exempt** (they reuse a slot), so this only ever
    /// surfaces for a route the device doesn't already hold.
    case storageFull
    /// A received object failed CRC validation before commit (see `OBCProtocol.md`
    /// → *CoC framing*). The object is rejected, never committed.
    case crcMismatch
    /// The device's `protocol_version` does not match `OBCProtocol.version`.
    /// Surfaced, not fatal.
    case protocolMismatch(expected: UInt16, found: UInt16)
}
