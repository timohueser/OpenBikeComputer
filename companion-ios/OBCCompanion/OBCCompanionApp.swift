import SwiftUI
import UIKit
import UserNotifications
import OBCDomain
import OBCTransport
import OBCUI
import OBCRouting

#if DEBUG
import OBCMock
#endif

/// Composition root. The single place allowed to choose a `DeviceTransport` conformer;
/// everything below `RootView` sees only the protocol.
///
/// CoreBluetooth lives only in `BLETransport`, and mock or panel code only inside `#if DEBUG`.
@main
struct OBCCompanionApp: App {
    #if DEBUG
    /// The launch surface, parsed once.
    private static let launchOptions = MockLaunchOptions.parse()
    /// The live control shared by the Debug transport, the dev panel and the HUD. Nil when the
    /// launch arguments force the real path.
    static let mockControl: MockControl? =
        launchOptions.useBLETransport ? nil : launchOptions.makeControl()
    #endif

    @MainActor private static let liveTransport = BLETransport()

    /// The notification tap router. Held here because `UNUserNotificationCenter.delegate` is a
    /// weak reference, so a delegate created inline would be released before the first tap.
    private static let notificationDelegate = UpdateNotificationDelegate()

    init() {
        // Field-guide nav chrome: the one global UIKit-appearance call the component kit needs.
        OBCNavigationChrome.apply()
        // Tapping an update notice must land on the firmware screen even from a cold launch, so
        // the delegate has to be in place before iOS delivers the pending response. Setting a
        // delegate asks for no permission and shows nothing.
        UNUserNotificationCenter.current().delegate = Self.notificationDelegate
        #if DEBUG
        // The screenshot capture must never catch a transition mid-flight. UIKit's switch covers
        // the presentation machinery SwiftUI drives underneath; the SwiftUI-side transaction
        // override lives on the root view below and covers implicit view-update animations.
        if Self.launchOptions.disableAnimations {
            UIView.setAnimationsEnabled(false)
        }
        // Log a DEBUG-only symbol at launch, so the mock-exclusion seam is exercised by a real
        // build and lands in the Debug binary, never the Release one.
        print("[OBC] debug build · mock seam: \(obcMockBuildMarker)")
        #endif
    }

    var body: some Scene {
        WindowGroup {
            RootView(
                transport: Self.makeTransport(),
                bondStore: Self.makeBondStore(),
                library: Self.makeLibraryStore(),
                photoLibrary: Self.makePhotoLibrary(),
                lastBikeType: Self.makeLastBikeTypeStore(),
                reachability: Self.makeReachability(),
                updateSurface: Self.makeUpdateSurfaceStore(),
                updateNotifier: SystemUpdateNotifier(),
                importAtLaunch: Self.launchImport(),
                firmwareDemoAtLaunch: Self.launchFirmwareDemo(),
                syncTiming: Self.launchSyncTiming(),
                placeName: Self.makePlaceName(),
                stopSearch: Self.makeStopSearch(),
                legRouter: Self.makeLegRouter())
            #if DEBUG
                .devMockOverlay(
                    control: Self.mockControl,
                    showPanelAtLaunch: Self.launchOptions.showDevPanel,
                    showGalleryAtLaunch: Self.launchOptions.showUIGallery,
                    hideHUD: Self.launchOptions.hideMockHUD
                )
                // The SwiftUI half of the animation switch: every state change in this subtree
                // lands unanimated, presentations included. A no-op without the flag.
                .transaction { transaction in
                    guard Self.launchOptions.disableAnimations else { return }
                    transaction.animation = nil
                    transaction.disablesAnimations = true
                }
            #endif
        }
        // This modifier is the `BGTaskScheduler` registration: SwiftUI does it before the app
        // finishes launching, which is the framework's hard requirement, and the identifier must
        // also be listed in `BGTaskSchedulerPermittedIdentifiers` or iOS traps at launch.
        // Everything else about the wake, the decision to say nothing included, is
        // `BackgroundUpdateRefresh.run()`.
        .backgroundTask(.appRefresh(BackgroundUpdateRefresh.identifier)) {
            await BackgroundUpdateRefresh.run()
        }
    }

    /// Mock runs start from Road on every launch, like their in-memory library.
    static func makeLastBikeTypeStore() -> LastBikeTypeStore {
        #if DEBUG
        if mockControl != nil, let defaults = UserDefaults(suiteName: "obc.mock") {
            defaults.removePersistentDomain(forName: "obc.mock")
            return LastBikeTypeStore(defaults: defaults)
        }
        #endif
        return LastBikeTypeStore()
    }

    /// Day ends take their locality's name. Mock runs stay offline and deterministic.
    static func makePlaceName() -> (@Sendable (Coordinate) async -> String?)? {
        #if DEBUG
        if mockControl != nil { return nil }
        #endif
        return PlaceNames.locality(at:)
    }

    /// Stops near a trip line come from Apple Maps. Fixture runs use the fixed stops near the
    /// fixture trips, offline and deterministic. A trip imported in any other Debug run needs the
    /// real search: the fixed stops lie in Wisconsin.
    static func makeStopSearch() -> any StopSearch {
        #if DEBUG
        if mockControl != nil, launchOptions.fixtures != nil { return MockStopSearch() }
        #endif
        return AppleMapsStopSearch()
    }

    /// The device's router over map cells fetched on demand. Fixture runs route offline with the
    /// mock, which `-OBCRouter` can make fail.
    static func makeLegRouter() -> any LegRouter {
        #if DEBUG
        if mockControl != nil, launchOptions.fixtures != nil { return MockLegRouter(failure: launchOptions.routerFailure) }
        #endif
        return CellRouter()
    }

    static func makeUpdateSurfaceStore() -> any UpdateSurfaceStore {
        #if DEBUG
        if mockControl != nil { return InMemoryUpdateSurfaceStore() }
        #endif
        return UserDefaultsUpdateSurfaceStore()
    }

    /// Debug defaults to the fixture-backed mock, because there is no BLE in the simulator, booted
    /// into whatever the launch arguments asked for. A forced flag, or Release, wires the real
    /// `BLETransport`. This is the only place a concrete transport is chosen.
    @MainActor static func makeTransport() -> any DeviceTransport {
        #if DEBUG
        if let mockControl { return MockTransport(control: mockControl) }
        #endif
        return Self.liveTransport
    }

    /// Hand bundled sample files to the import path at launch, exactly as a Files pick would, so
    /// the UI tests and demos run the real decoders. Debug-only, like every launch argument.
    static func launchImport() -> [(data: Data, fileName: String)] {
        #if DEBUG
        guard let kind = launchOptions.importSample else { return [] }
        return SampleRouteFile.files(kind)
        #else
        return []
        #endif
    }

    /// A pre-staged sample update for the firmware screen, so the flow can be captured without a
    /// real container in Files. The argument can also fire the transfer. Debug-only, and only under
    /// the mock.
    static func launchFirmwareDemo() -> (data: Data, autoSend: Bool)? {
        #if DEBUG
        guard let stage = launchOptions.firmwareDemo, mockControl != nil else { return nil }
        return (SampleFirmwareFile.container(), stage == .sending)
        #else
        return nil
        #endif
    }

    /// The confirmation hold, sync half: the coordinator normally returns the top-bar check to
    /// idle two seconds after a sync lands, and drops the confirm line a minute later. Both beats
    /// are real product behaviour and both are wall clocks, so an automated capture aiming at one
    /// is racing. The flag parks them instead of expiring them, and ordinary runs get the shipped
    /// timing.
    static func launchSyncTiming() -> RideSyncCoordinator.Timing {
        #if DEBUG
        if launchOptions.holdConfirmations {
            return RideSyncCoordinator.Timing(syncDoneHold: .seconds(3_600), syncedLineHold: .seconds(3_600))
        }
        #endif
        return RideSyncCoordinator.Timing()
    }

    /// The same hook's upload half: the upload sheet dismisses itself a few seconds after it says
    /// "On the device", so a finished-upload screenshot has a very short window to exist in. Read
    /// by the two upload seams in `ScreenHosts`: they sit deep in the tree, and the launch surface
    /// is this module's business, so they ask here rather than have a timing threaded down.
    static func launchUploadTiming() -> UploadSheetModel.Timing {
        #if DEBUG
        if launchOptions.holdConfirmations {
            return UploadSheetModel.Timing(doneAutoDismiss: .seconds(3_600))
        }
        #endif
        return UploadSheetModel.Timing()
    }

    /// The phone-side library. Mock runs stay in-memory, because every scenario-driven launch must
    /// start from its fixtures alone and not from whatever a previous run saved. The real path
    /// persists to Application Support.
    static func makeLibraryStore() -> any LibraryStore {
        #if DEBUG
        if let mockControl {
            let store = InMemoryLibraryStore()
            // The Planned list is library-first: fixture routes exist as phone-side saves, with
            // `deviceObjectID` marking the ones the mock device also holds.
            mockControl.seedLibrary(into: store)
            // This scenario's premise is "everything already synced", and the synced set is the
            // library's, so it is seeded here and the first sync reports up to date.
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

    /// The simulator has no photos worth placing, so mock runs draw their own.
    static func makePhotoLibrary() -> any PhotoLibrary {
        #if DEBUG
        if mockControl != nil {
            return MockPhotoLibrary(
                access: launchOptions.photoAccess ?? .notDetermined, lastPhotoGone: launchOptions.photoGone)
        }
        #endif
        return PhotoKitLibrary()
    }

    /// The reachability seam behind the basemap. The real path watches `NWPathMonitor`, and a
    /// launch argument pins it for automation. Debug-only, like every launch argument.
    static func makeReachability() -> any NetworkReachability {
        #if DEBUG
        if let online = launchOptions.networkOnline { return ConstantReachability(online) }
        #endif
        return PathMonitorReachability()
    }

    /// The bond record behind the launch branch. Mock runs read it from the scenario, so the dev
    /// panel can replay first-run pairing; the real path persists it in `UserDefaults`.
    static func makeBondStore() -> any BondStore {
        #if DEBUG
        if let mockControl { return MockBondStore(control: mockControl) }

        #endif
        return UserDefaultsBondStore()
    }
}
