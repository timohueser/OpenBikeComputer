import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI
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
    #if DEBUG
    /// The B1P launch surface, parsed once (`-OBCScenario …`, see CLAUDE.md).
    private static let launchOptions = MockLaunchOptions.parse()
    /// The live control shared by the Debug transport, the dev panel, and the
    /// HUD — `nil` when `-OBCTransport ble` forces the real path.
    static let mockControl: MockControl? =
        launchOptions.useBLETransport ? nil : launchOptions.makeControl()
    #endif

    init() {
        // Field-guide nav chrome (serif large titles, parchment bar) — the one
        // global UIKit-appearance call the B11 kit needs (§9 "Nav Bar").
        OBCNavigationChrome.apply()
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
            #if DEBUG
                .devMockOverlay(
                    control: Self.mockControl,
                    showPanelAtLaunch: Self.launchOptions.showDevPanel,
                    showGalleryAtLaunch: Self.launchOptions.showUIGallery
                )
            #endif
        }
    }

    /// Debug defaults to the fixture-backed mock (no BLE in the simulator),
    /// booted into whatever the launch arguments asked for; `-OBCTransport ble`
    /// (or Release, always) wires the real `BLETransport`. This is the **only**
    /// place a concrete transport is chosen — everything below sees
    /// `any DeviceTransport`.
    static func makeTransport() -> any DeviceTransport {
        #if DEBUG
        if let mockControl { return MockTransport(control: mockControl) }
        #endif
        return BLETransport()
    }
}
