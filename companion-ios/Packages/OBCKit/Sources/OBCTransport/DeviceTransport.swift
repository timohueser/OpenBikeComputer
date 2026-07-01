import OBCDomain

/// The spine of the app. **Every view model depends only on this protocol** —
/// never on CoreBluetooth. Two conformers exist:
///
///   • `BLETransport`  (real, B1) — CoreBluetooth + the BLEChannel byte layer.
///   • `MockTransport` (fake, #if DEBUG) — fixtures + fault injection.
///
/// B0 gives it a single method so the seam is real and testable; B1 grows it
/// into the full route/ride/device surface. `Sendable` so conformers can be
/// actors or value types and still cross concurrency domains.
public protocol DeviceTransport: Sendable {
    /// Identity of the connected device.
    func fetchDeviceInfo() async throws -> DeviceInfo
}
