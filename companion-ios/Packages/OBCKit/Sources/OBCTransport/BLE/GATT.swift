#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth

/// The GATT service/characteristic map `BLETransport` discovers — the control
/// plane of `obc-ble-interface-spec.md` (§3, **pinned by firmware S0 / PR #279**;
/// summarized for iOS in `OBCProtocol.md`).
///
/// The OBC Control UUIDs use the random base `3C92XXXX-9916-4EBA-ABC2-342FE08F6B10`
/// where the 16-bit `XXXX` block selects the entity (`0000` = the service, `000N` =
/// characteristic N). Custom UUIDs must not derive from the Bluetooth SIG base —
/// which is why the earlier `0BC0…` placeholders were replaced, not ratified.
///
/// `CBUUID` is immutable but not `Sendable`-audited; `nonisolated(unsafe)` states
/// the (true) invariant that these constants are safe to share.
///
/// `public` so host tooling can reuse the same pinned UUIDs rather than a copy that could drift.
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
    /// Results for authenticated BLE imperative commands; v4 object results use `objectControl`.
    nonisolated(unsafe) public static let status = CBUUID(string: "3C920002-9916-4EBA-ABC2-342FE08F6B10")
    // `0003` (`objectStore`) retired in v2 and must not be reused.
    /// The Config object, whole-blob read + write (incl. rename, Delta 1) — spec §7.3.
    nonisolated(unsafe) public static let config = CBUUID(string: "3C920004-9916-4EBA-ABC2-342FE08F6B10")
    // `0005` (`transferControl`) retired with protocol v2 and must not be reused.
    // `0006` (`diagnostics`) retired in v2 and must not be reused.
    /// The dynamically-assigned L2CAP CoC PSM the app opens the channel on.
    nonisolated(unsafe) public static let psm = CBUUID(string: "3C920007-9916-4EBA-ABC2-342FE08F6B10")
    /// Protocol-v4 wire major as one little-endian `u16`, readable without encryption. Store
    /// identity comes from `LIST`, never from this transport fact.
    nonisolated(unsafe) public static let protocolVersion = CBUUID(string: "3C920008-9916-4EBA-ABC2-342FE08F6B10")
    /// Protocol-v4 control records: one authenticated Write Request per request and one confirmed
    /// indication per result (`FLAT_Store_Protocol.md` §5.1). This is the only store control
    /// characteristic used by the v4 client.
    nonisolated(unsafe) public static let objectControl = CBUUID(string: "3C920009-9916-4EBA-ABC2-342FE08F6B10")

}
#endif
