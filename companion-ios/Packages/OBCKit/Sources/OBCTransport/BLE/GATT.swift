#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth

/// The GATT service/characteristic map `BLETransport` discovers — the control
/// plane of `OBCProtocol.md`.
///
/// > **Provisional custom UUIDs — pin from firmware `S0`.** The SIG services (DIS
/// > `0x180A`, BAS `0x180F`) are fixed. The **OBC Control** service + characteristic
/// > 128-bit UUIDs are firmware-owned (`OBCProtocol.md` explicitly forbids inventing
/// > them); the values below are placeholders so `BLETransport` compiles and can be
/// > brought up the moment `A4` lands. Replace these — and only these — from the spec.
///
/// `CBUUID` is immutable but not `Sendable`-audited; `nonisolated(unsafe)` states
/// the (true) invariant that these constants are safe to share.
enum GATT {
    // MARK: SIG services (fixed)
    nonisolated(unsafe) static let deviceInformation = CBUUID(string: "180A")
    nonisolated(unsafe) static let battery = CBUUID(string: "180F")

    // DIS characteristics (fixed).
    nonisolated(unsafe) static let firmwareRevision = CBUUID(string: "2A26")
    nonisolated(unsafe) static let hardwareRevision = CBUUID(string: "2A27")
    nonisolated(unsafe) static let serialNumber = CBUUID(string: "2A25")
    // BAS characteristic (fixed).
    nonisolated(unsafe) static let batteryLevel = CBUUID(string: "2A19")

    // MARK: OBC Control (custom — PROVISIONAL, pin from S0)
    nonisolated(unsafe) static let obcControlService = CBUUID(string: "0BC00000-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let command = CBUUID(string: "0BC00001-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let status = CBUUID(string: "0BC00002-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let rideList = CBUUID(string: "0BC00003-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let config = CBUUID(string: "0BC00004-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let transferControl = CBUUID(string: "0BC00005-0000-1000-8000-00805F9B34FB")
    nonisolated(unsafe) static let diagnostics = CBUUID(string: "0BC00006-0000-1000-8000-00805F9B34FB")
    /// The dynamically-assigned L2CAP CoC PSM the app opens the channel on.
    nonisolated(unsafe) static let psm = CBUUID(string: "0BC00007-0000-1000-8000-00805F9B34FB")
    /// `protocol_version` (may also come via DIS) — read on connect for the
    /// version check (`OBCProtocol.md` → *Versioning*).
    nonisolated(unsafe) static let protocolVersion = CBUUID(string: "0BC00008-0000-1000-8000-00805F9B34FB")
}
#endif
