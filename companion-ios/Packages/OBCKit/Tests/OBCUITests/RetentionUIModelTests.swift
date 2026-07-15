import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The S7 view-model seams (epic #638): the Settings default persists and seeds
/// the main model; the upload sheet's `.ready` confirm carries the chosen level;
/// route detail gates and formats its Auto-delete row; the main model edits and
/// badges retention. Transport-level reconcile/push is `RouteRetentionReconcileTests`.
@MainActor @Suite struct RetentionUIModelTests {
    private func waitFor(
        _ what: String, timeout: Duration = .seconds(30), _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline { Issue.record("timed out waiting for \(what)"); return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    private func makeMain(
        _ scenario: Scenario = .happyPath,
        library: any LibraryStore = InMemoryLibraryStore(),
        defaults: any RetentionDefaultsStore = InMemoryRetentionDefaultsStore(),
        now: @escaping () -> Date = Date.init
    ) -> (MainScreenModel, MockControl, any LibraryStore) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        control.throughputBytesPerSec = 50_000_000
        control.seedLibrary(into: library)
        let model = MainScreenModel(
            transport: MockTransport(control: control), library: library,
            retentionDefaults: defaults, now: now)
        return (model, control, library)
    }

    private func record(_ library: any LibraryStore, objectID: UInt16) -> PlannedRouteRecord? {
        library.plannedRoutes().first { $0.deviceLink?.objectID == DeviceObjectID(objectID) }
    }

    // MARK: Settings default ↔ main model (shared store)

    /// The Settings picker persists through the store and the main model reads the
    /// same instance — a change in Settings seeds the next upload's default.
    @Test func settingsDefaultSeedsTheMainModel() {
        let store = InMemoryRetentionDefaultsStore()
        let (main, _, _) = makeMain(defaults: store)
        let settings = SettingsModel(
            transport: MockTransport(control: MockControl(scenario: .happyPath)),
            bondStore: MockBondStore(control: MockControl(scenario: .happyPath)),
            retentionDefaults: store)

        #expect(settings.defaultRetention == .twoWeeks)   // the documented default
        #expect(main.defaultRetention == .twoWeeks)

        settings.setDefaultRetention(.oneMonth)
        #expect(settings.defaultRetention == .oneMonth)
        #expect(store.loadDefaultRetention() == .oneMonth)
        #expect(main.defaultRetention == .oneMonth)        // the main model sees it live
    }

    /// An unchanged pick is a no-op (no needless write).
    @Test func settingDefaultToTheSameLevelIsANoOp() {
        let settings = SettingsModel(
            transport: MockTransport(control: MockControl(scenario: .happyPath)),
            bondStore: MockBondStore(control: MockControl(scenario: .happyPath)),
            retentionDefaults: InMemoryRetentionDefaultsStore(.oneWeek))
        settings.setDefaultRetention(.oneWeek)
        #expect(settings.defaultRetention == .oneWeek)
    }

    // MARK: Upload sheet — the .ready confirm carries the choice

    private func uploadModel(supportsRetention: Bool, retention: Retention)
        -> (UploadSheetModel, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 50_000_000
        let blob = RouteBlob(
            summary: RouteSummary(id: RouteID("u"), name: "R", distanceMeters: 1, elevationGainMeters: 1),
            waypoints: [], payload: Data(count: 10_000))
        let model = UploadSheetModel(
            transport: MockTransport(control: control), blob: blob, deviceName: "Trailhead",
            retention: retention, supportsRetention: supportsRetention,
            timing: .init(doneAutoDismiss: .milliseconds(20)))
        return (model, control)
    }

    /// A retention-capable device opens on `.ready` (the pre-transfer confirm) and
    /// `start()` does **not** begin the transfer — the chosen level is set before
    /// the push.
    @Test func capableUploadHoldsOnReadyUntilBegun() async {
        let (model, _) = uploadModel(supportsRetention: true, retention: .twoWeeks)
        #expect(model.phase == .ready)
        model.start()
        try? await Task.sleep(for: .milliseconds(60))
        #expect(model.phase == .ready)          // still waiting on the Upload button
        #expect(model.progress.bytesDone == 0)
    }

    /// Begin from `.ready` and the chosen retention rides to `onCompleted`
    /// (S6's post-commit push sends it).
    @Test func beginUploadCarriesTheChosenRetention() async {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 50_000_000
        let blob = RouteBlob(
            summary: RouteSummary(id: RouteID("u"), name: "R", distanceMeters: 1, elevationGainMeters: 1),
            waypoints: [], payload: Data(count: 10_000))
        var landed: Retention?
        let model = UploadSheetModel(
            transport: MockTransport(control: control), blob: blob, deviceName: "Trailhead",
            retention: .twoWeeks, supportsRetention: true,
            timing: .init(doneAutoDismiss: .milliseconds(20)),
            onCompleted: { _, _, retention in landed = retention })

        model.start()
        model.selectRetention(.oneMonth)        // the rider changes it in the confirm
        #expect(model.retention == .oneMonth)
        model.beginUpload()
        await waitFor("completion") { model.phase == .done }
        #expect(landed == .oneMonth)
    }

    /// A device without capability skips the confirm: `start()` begins the
    /// transfer immediately (the prior behaviour), the row hidden.
    @Test func incapableUploadStartsImmediately() async {
        let (model, _) = uploadModel(supportsRetention: false, retention: .twoWeeks)
        #expect(model.phase == .uploading)
        model.start()
        await waitFor("progress") { model.progress.bytesDone > 0 || model.phase == .done }
    }

    // MARK: Route detail — gating + expiry line

    private func detailModel(
        onDevice: Bool, supportsRetention: Bool, retention: Retention?, expiresAt: Date?,
        onEdit: ((Retention) -> Void)? = nil, now: @escaping () -> Date = Date.init
    ) -> RouteDetailModel {
        RouteDetailModel(
            transport: MockTransport(control: MockControl(scenario: .happyPath)),
            dressing: .planned(RouteSummary(
                id: RouteID("r"), name: "R", distanceMeters: 1, elevationGainMeters: 1)),
            provenCommittedCRC: onDevice ? 42 : nil,
            retention: retention, deviceExpiresAt: expiresAt,
            supportsRetention: supportsRetention, onEditRetention: onEdit, now: now)
    }

    @Test func detailShowsTheRowOnlyWhenOnDeviceAndCapable() {
        #expect(detailModel(onDevice: true, supportsRetention: true, retention: .twoWeeks, expiresAt: nil)
            .showsRetentionRow)
        #expect(!detailModel(onDevice: false, supportsRetention: true, retention: .twoWeeks, expiresAt: nil)
            .showsRetentionRow)
        #expect(!detailModel(onDevice: true, supportsRetention: false, retention: .twoWeeks, expiresAt: nil)
            .showsRetentionRow)
    }

    @Test func detailRowValueDefaultsToNeverWhenUnset() {
        #expect(detailModel(onDevice: true, supportsRetention: true, retention: nil, expiresAt: nil)
            .retentionValue == .never)
    }

    @Test func detailExpiryLineOmittedWhenNoDeviceExpiry() {
        #expect(detailModel(onDevice: true, supportsRetention: true, retention: .twoWeeks, expiresAt: nil)
            .expiryLine == nil)
    }

    @Test func detailExpiryLineFormatsDeviceTruth() {
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        let model = detailModel(
            onDevice: true, supportsRetention: true, retention: .oneWeek,
            expiresAt: now.addingTimeInterval(2 * 86_400 + 3_600), now: { now })
        #expect(model.expiryLine == "Expires in 2 days")
    }

    @Test func detailEditUpdatesValueAndNotifies() {
        var edited: Retention?
        let model = detailModel(
            onDevice: true, supportsRetention: true, retention: .twoWeeks, expiresAt: nil,
            onEdit: { edited = $0 })
        model.editRetention(.twoMonths)
        #expect(model.retentionValue == .twoMonths)
        #expect(edited == .twoMonths)
    }

    // MARK: Main model — edit push, badge, explicit-level upload

    /// Editing a route's retention from the detail, connected, pushes it now and
    /// stores the desired level.
    @Test func setRouteRetentionPushesWhenConnected() async {
        let (model, control, library) = makeMain()
        model.start()
        await waitFor("the lists") { model.loadState == .loaded && model.connectedScope != nil }
        // Route 7's device level is one week; pick two months from the detail.
        let id = record(library, objectID: 7)!.id
        model.setRouteRetention(id, .twoMonths)
        await waitFor("the push") { control.routeRetention(for: DeviceObjectID(7)) == .twoMonths }
        #expect(library.plannedRoutes().first { $0.id == id }?.retention == .twoMonths)
    }

    /// The near-expiry fixture (kettle-moraine: one week, last used 5 d ago →
    /// ~2 d) shows the card badge; a far-off route shows none.
    @Test func expiryBadgeShowsForANearExpiryRouteOnly() async {
        let (model, _, library) = makeMain()
        model.start()
        await waitFor("the reconcile lands device expiry") {
            record(library, objectID: 7)?.deviceExpiresAt != nil
        }
        let nearID = record(library, objectID: 7)!.id     // one week / 5 d ago → ~2 d
        let farID = record(library, objectID: 12)!.id     // one month / 3 d ago → weeks out
        #expect(model.expiryBadge(for: nearID)?.hasPrefix("Expires") == true)
        #expect(model.expiryBadge(for: farID) == nil)
    }

    /// A `notOnDevice` route never badges, even with a stale `deviceExpiresAt`.
    @Test func expiryBadgeHidesForNotOnDeviceRoutes() async {
        let (model, _, library) = makeMain()
        model.start()
        await waitFor("the lists") { model.loadState == .loaded }
        // A library-only route (never uploaded) — no badge regardless.
        let planned = library.plannedRoutes().first { $0.deviceLink == nil }!
        #expect(model.expiryBadge(for: planned.id) == nil)
    }

    /// An upload with an explicit chosen level pushes that level, over the default.
    @Test func uploadWithExplicitRetentionPushesTheChoice() async {
        let (model, control, library) = makeMain()
        model.start()
        await waitFor("the lists") { model.loadState == .loaded && model.connectedScope != nil }

        let planned = library.plannedRoutes().first { $0.deviceLink == nil }!
        let payload = Data([9, 9, 9])
        let handle = MockTransport(control: control).uploadRoute(
            RouteBlob(summary: planned.summary, waypoints: planned.route.waypoints, payload: payload))
        guard await handle.outcome == .completed, let objectID = await handle.assignedObjectID else {
            Issue.record("mock upload must commit"); return
        }
        model.markRouteUploaded(planned.id, objectID: objectID, crc32: CRC32.checksum(payload),
            retention: .twoMonths)
        await waitFor("the explicit-level push") { control.routeRetention(for: objectID) == .twoMonths }
        #expect(library.plannedRoutes().first { $0.id == planned.id }?.retention == .twoMonths)
    }
}
