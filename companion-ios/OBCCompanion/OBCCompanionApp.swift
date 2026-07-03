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
            RootView(
                transport: Self.makeTransport(),
                bondStore: Self.makeBondStore(),
                library: Self.makeLibraryStore(),
                importAtLaunch: Self.launchImport()
            )
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

    /// The `-OBCImportSample [gpx|tcx|bad]` hook: hand a bundled sample file to
    /// the import path at launch, exactly as a Files pick would — the E1/H4/H5
    /// XCUITests and demos run the real decoders. Debug-only, like every launch arg.
    static func launchImport() -> (data: Data, fileName: String)? {
        #if DEBUG
        guard let kind = launchOptions.importSample else { return nil }
        return SampleRouteFile.data(kind).map { ($0, SampleRouteFile.fileName(kind)) }
        #else
        return nil
        #endif
    }

    /// The phone-side library (B1S). Mock runs stay **in-memory** — every
    /// scenario-driven launch (XCUITests, previews, demos) must start from its
    /// fixtures alone, not whatever a previous run saved. The real path
    /// persists to Application Support.
    static func makeLibraryStore() -> any LibraryStore {
        #if DEBUG
        if let mockControl {
            let store = InMemoryLibraryStore()
            // The Planned list is library-first (#289): fixture routes exist as
            // phone-side saves, with `deviceObjectID` marking the ones the mock
            // device also holds (the C1 badge + `listRoutes()` reconcile).
            mockControl.seedLibrary(into: store)
            // H9's premise is "everything already synced" — the synced set is
            // the library's (B1S), so the scenario seeds it here and the FIRST
            // sync reports up to date.
            if mockControl.scenario == .syncUpToDate {
                for entry in mockControl.fixtures.rides {
                    store.saveRide(entry.ride())
                    store.markRideSynced(entry.summary.id)
                }
            }
            return store
        }
        #endif
        return FileLibraryStore.standard()
    }

    /// The bond record behind the B2 launch branch. Mock runs read it from the
    /// scenario (`MockControl.bonded` — flip it in the dev panel to replay
    /// first-run pairing); the real path persists it in `UserDefaults`.
    static func makeBondStore() -> any BondStore {
        #if DEBUG
        if let mockControl { return MockBondStore(control: mockControl) }
        #endif
        return UserDefaultsBondStore()
    }
}
