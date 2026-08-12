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
    /// Typed device → app notifications (`StatusMessage`, incl. the download
    /// announce as `msg = 4` and weather wake-up as `msg = 5`) — the **sole**
    /// device → app channel, spec §4.3.
    nonisolated(unsafe) public static let status = CBUUID(string: "3C920002-9916-4EBA-ABC2-342FE08F6B10")
    // `0003` (`objectStore`) is **retired in v2** — the change signal is
    // `storeChanged` alone — and must not be reused.
    /// The Config object, whole-blob read + write (incl. rename, Delta 1) — spec §7.3.
    nonisolated(unsafe) public static let config = CBUUID(string: "3C920004-9916-4EBA-ABC2-342FE08F6B10")
    /// Open / abort a CoC transfer (`TransferControl`) — spec §4.2. **Write-only in
    /// v2** (no CCCD): the download announce it once notified now rides `status`.
    nonisolated(unsafe) public static let transferControl = CBUUID(string: "3C920005-9916-4EBA-ABC2-342FE08F6B10")
    // `0006` (`diagnostics`) is **retired in v2** (it returned 0 bytes; real
    // diagnostics cross the CoC as object type 4, spec §7.5) — must not be reused.
    /// The dynamically-assigned L2CAP CoC PSM the app opens the channel on.
    nonisolated(unsafe) public static let psm = CBUUID(string: "3C920007-9916-4EBA-ABC2-342FE08F6B10")
    /// `version u16 · store_epoch u32 · obcm_version u8 · feature_bits u32` LE — read on connect
    /// for the version check (spec §1), the store-epoch identity, and the capability word (WX3);
    /// **length-driven**, readable without encryption.
    nonisolated(unsafe) public static let protocolVersion = CBUUID(string: "3C920008-9916-4EBA-ABC2-342FE08F6B10")

    // MARK: OBC Weather Request (spec §11, WX3 / #1188)
    // A random 128-bit base of its own, deliberately *not* a block inside the OBC Control base:
    // iOS matches the advertisement on this UUID alone, so the two services must be independently
    // advertisable. This service replaces OBC Control in the one legacy advertisement while a
    // request is pending; **both** always exist in the connected GATT database — advertising a
    // service the connected database lacks is exactly the trap this avoids.
    // The base has never shipped, so `0001` is a first assignment, not a reuse.
    nonisolated(unsafe) public static let weatherRequestService =
        CBUUID(string: "B3B60000-33B4-4F02-A5FF-E5954D54B5AA")
    /// The read-only, **authenticated** request context — 52 LE bytes, `WeatherRequestContext`.
    /// Authenticated because the value describes where the rider is: an unbonded peer that connects
    /// to the advertisement gets an ATT security error, and does not consume the pending request
    /// either.
    nonisolated(unsafe) public static let weatherRequestContext =
        CBUUID(string: "B3B60001-33B4-4F02-A5FF-E5954D54B5AA")
}
#endif
