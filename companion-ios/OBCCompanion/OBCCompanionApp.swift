import SwiftUI
import OBCDomain
import OBCTransport
#if DEBUG
import OBCMock
#endif

/// Composition root. The single place allowed to *choose* a `DeviceTransport`
/// conformer — everything below `RootView` sees only the protocol.
///
/// The golden rule (see companion-ios/CLAUDE.md): CoreBluetooth lives only in
/// `BLETransport`; mock/panel code only inside `#if DEBUG`.
@main
struct OBCCompanionApp: App {
    init() {
        #if DEBUG
        // Log a DEBUG-only symbol at launch so the mock-exclusion seam is exercised
        // by a real build and lands in the Debug binary — but never the Release one
        // (B0 acceptance). See CLAUDE.md → "Prove the seam".
        print("[OBC] debug build · mock seam: \(obcMockBuildMarker)")
        #endif
    }

    var body: some Scene {
        WindowGroup {
            RootView(transport: Self.makeTransport())
        }
    }

    /// Debug defaults to the fixture-backed mock (no BLE in the simulator);
    /// Release wires the real `BLETransport` (B1). This is the **only** place a
    /// concrete transport is chosen — everything below sees `any DeviceTransport`.
    static func makeTransport() -> any DeviceTransport {
        #if DEBUG
        return MockTransport()
        #else
        return BLETransport()
        #endif
    }
}
