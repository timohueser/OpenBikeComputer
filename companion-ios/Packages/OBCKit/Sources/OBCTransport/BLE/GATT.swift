#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth

/// The GATT service/characteristic map `BLETransport` discovers — the control
/// plane of `obc-ble-interface-spec.md` (§3, **pinned by firmware S0 / PR #279**;
/// mirrored in `OBCProtocol.md`).
///
/// The OBC Control UUIDs use the random base `3C92XXXX-9916-4EBA-ABC2-342FE08F6B10`
/// where the 16-bit `XXXX` block selects the entity (`0000` = the service, `000N` =
/// characteristic N). Custom UUIDs must not derive from the Bluetooth SIG base —
/// which is why the earlier `0BC0…` placeholders were replaced, not ratified.
///
/// `CBUUID` is immutable but not `Sendable`-audited; `nonisolated(unsafe)` states
/// the (true) invariant that these constants are safe to share.
///
/// `public` so host tooling that reuses the transport primitives — the A5 echo
/// harness / A9 soak rig (`EchoHarness`), which drives its own `CBCentralManager`
/// — scans + discovers against the *same* pinned UUIDs, not a copy that could drift.
public enum GATT {
    // MARK: SIG services (fixed)
    nonisolated(unsafe) public static let deviceInformation = CBUUID(string: "180A")
    nonisolated(unsafe) public static let battery = CBUUID(string: "180F")

    // DIS characteristics (fixed).
    nonisolated(unsafe) public static let firmwareRevision = CBUUID(string: "2A26")
    nonisolated(unsafe) public static let hardwareRevision = CBUUID(string: "2A27")
    nonisolated(unsafe) public static let serialNumber = CBUUID(string: "2A25")
    // BAS characteristic (fixed).
    nonisolated(unsafe) public static let batteryLevel = CBUUID(string: "2A19")

    // MARK: OBC Control (custom — pinned by S0, spec §3.3)
    nonisolated(unsafe) public static let obcControlService = CBUUID(string: "3C920000-9916-4EBA-ABC2-342FE08F6B10")
    /// Small imperative commands (delete object, …) — spec §4.4.
    nonisolated(unsafe) public static let command = CBUUID(string: "3C920001-9916-4EBA-ABC2-342FE08F6B10")
    /// Typed device → app notifications (`StatusMessage`) — spec §4.3.
    nonisolated(unsafe) public static let status = CBUUID(string: "3C920002-9916-4EBA-ABC2-342FE08F6B10")
    /// The store digest (`ObjectStoreDigest`, read + notify) — spec §4.5. Full
    /// route/ride lists are CoC objects (they outgrow the 512-byte ATT cap).
    nonisolated(unsafe) public static let objectStore = CBUUID(string: "3C920003-9916-4EBA-ABC2-342FE08F6B10")
    /// The Config object, whole-blob read + write (incl. rename, Delta 1) — spec §7.3.
    nonisolated(unsafe) public static let config = CBUUID(string: "3C920004-9916-4EBA-ABC2-342FE08F6B10")
    /// Open / resume / abort a CoC transfer (`TransferControl`) — spec §4.2.
    nonisolated(unsafe) public static let transferControl = CBUUID(string: "3C920005-9916-4EBA-ABC2-342FE08F6B10")
    /// Reserved — diagnostics cross the CoC as object type 4 (spec §7.5).
    nonisolated(unsafe) public static let diagnostics = CBUUID(string: "3C920006-9916-4EBA-ABC2-342FE08F6B10")
    /// The dynamically-assigned L2CAP CoC PSM the app opens the channel on.
    nonisolated(unsafe) public static let psm = CBUUID(string: "3C920007-9916-4EBA-ABC2-342FE08F6B10")
    /// `protocol_version` (u16 LE) — read on connect for the version check
    /// (spec §1); readable without encryption.
    nonisolated(unsafe) public static let protocolVersion = CBUUID(string: "3C920008-9916-4EBA-ABC2-342FE08F6B10")
}
#endif
