import Foundation
import Testing
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Route auto-expiry, the app half (epic #638 S6): the connect prologue stamps the
/// device clock and settles the capability flag; the badge reconcile lands the
/// device's expiry truth on each record and pushes the desired retention where it
/// diverges; an upload opts the route into the default and pushes it. All against
/// `OBCMock` — the same host-side path S7's UI tests will run on.
@MainActor @Suite struct RouteRetentionReconcileTests {
    private func makeModel(
        _ scenario: Scenario = .happyPath,
        library: any LibraryStore = InMemoryLibraryStore()
    ) -> (MainScreenModel, MockControl, any LibraryStore) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        control.throughputBytesPerSec = 50_000_000  // uploads finish fast in-test
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        return (model, control, library)
    }

    private func waitFor(
        _ what: String, timeout: Duration = .seconds(30), _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline { Issue.record("timed out waiting for \(what)"); return }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    private func record(_ library: any LibraryStore, objectID: UInt16) -> PlannedRouteRecord? {
        library.plannedRoutes().first { $0.deviceLink?.objectID == DeviceObjectID(objectID) }
    }

    // MARK: Connect-time clock stamp + capability

    /// Every connect stamps the device's trusted wall clock (spec §4.4 cmd 5) and
    /// settles `supportsRetention` to true against expiry-capable firmware.
    @Test func connectStampsTheClockAndReportsCapability() async {
        let (model, control, _) = makeModel()
        model.start()
        await waitFor("the clock stamp") { !control.setClockSamples.isEmpty }
        #expect(model.supportsRetention)
        // The sample carries a plausible in-range now.
        #expect(control.setClockSamples.first!.utcSeconds >= 1_577_836_800)
    }

    /// Old firmware answers `setClock` `unknown`: the capability drops, the clock
    /// stamp is still *sent* (the device just can't honour it), and **no**
    /// retention command follows — the gated state S7 hides UI behind.
    @Test func oldFirmwareDropsCapabilityAndPushesNoRetention() async {
        let (model, control, library) = makeModel(.oldFirmware)
        // Desire a divergent level on a device-held route — it must NOT push.
        if var r = record(library, objectID: 7) { r.retention = .twoMonths; library.savePlannedRoute(r) }

        model.start()
        await waitFor("the (unsupported) stamp") { !control.setClockSamples.isEmpty }
        await waitFor("the lists") { model.loadState == .loaded && model.connection == .connected }
        try? await Task.sleep(for: .milliseconds(100))  // give a wrong push a beat to land

        #expect(model.supportsRetention == false)
        // The device kept its fixture default — nothing was pushed.
        #expect(control.routeRetention(for: DeviceObjectID(7)) == .oneWeek)
    }

    // MARK: Reconcile-push matrix

    /// A desired level that diverges from the device's pushes exactly once and
    /// lands on the device.
    @Test func aDivergingDesiredLevelPushesOnce() async {
        let (model, control, library) = makeModel()
        if var r = record(library, objectID: 7) { r.retention = .twoMonths; library.savePlannedRoute(r) }

        model.start()
        await waitFor("the retention push") { control.routeRetention(for: DeviceObjectID(7)) == .twoMonths }
        #expect(control.routeRetention(for: DeviceObjectID(7)) == .twoMonths)
    }

    /// A `nil` desired level never pushes (invariant 6 — a route uploaded before
    /// expiry existed migrates as "not set" and can't be surprise-deleted). The
    /// device keeps its own level; the record still learns the device's truth.
    @Test func aNilDesiredLevelNeverPushes() async {
        let (model, control, library) = makeModel()
        // Route 7 desires two months (drives a push we can wait on); route 12 is
        // left nil (its seeded desired retention) — it must stay put.
        if var r = record(library, objectID: 7) { r.retention = .twoMonths; library.savePlannedRoute(r) }

        model.start()
        await waitFor("the route-7 push (reconcile ran)") {
            control.routeRetention(for: DeviceObjectID(7)) == .twoMonths
        }
        // Route 12 had no desired level — its device level is untouched…
        #expect(record(library, objectID: 12)?.retention == nil)
        #expect(control.routeRetention(for: DeviceObjectID(12)) == .oneMonth)
        // …and the device's expiry truth landed on the record (display-only).
        #expect(record(library, objectID: 12)?.deviceRetention == .oneMonth)
        #expect(record(library, objectID: 12)?.deviceExpiresAt != nil)
    }

    /// A desired level that already equals the device's pushes nothing new — the
    /// device level is unchanged and no divergence remains.
    @Test func aMatchingDesiredLevelIsANoOp() async {
        let (model, control, library) = makeModel()
        // Route 12's device level is one month; desire exactly that.
        if var r = record(library, objectID: 12) { r.retention = .oneMonth; library.savePlannedRoute(r) }

        model.start()
        await waitFor("the lists") { model.loadState == .loaded && model.connectedScope != nil }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(control.routeRetention(for: DeviceObjectID(12)) == .oneMonth)
        #expect(record(library, objectID: 12)?.deviceRetention == .oneMonth)
    }

    // MARK: Upload push

    /// After a route upload commits, the app opts the route into the default
    /// retention and pushes it — so a fresh route gets its expiry without a second
    /// upload — and records that desired level.
    @Test func anUploadOptsIntoTheDefaultAndPushesIt() async {
        let (model, control, library) = makeModel()
        model.start()
        await waitFor("the lists") { model.loadState == .loaded && model.connectedScope != nil }

        // A library route the device does not yet hold (no deviceObjectID fixture).
        let planned = library.plannedRoutes().first { $0.deviceLink == nil }!
        #expect(planned.retention == nil)
        let payload = Data([1, 2, 3])
        let handle = MockTransport(control: control).uploadRoute(
            RouteBlob(summary: planned.summary, waypoints: planned.route.waypoints, payload: payload))
        guard await handle.outcome == .completed, let objectID = await handle.assignedObjectID else {
            Issue.record("mock upload must commit and assign an id"); return
        }
        model.markRouteUploaded(planned.id, objectID: objectID, crc32: CRC32.checksum(payload))

        await waitFor("the default retention push") {
            control.routeRetention(for: objectID) == .appDefault
        }
        #expect(control.routeRetention(for: objectID) == .appDefault)
        // The upload opted the record into the default (desired level recorded).
        #expect(library.plannedRoutes().first { $0.id == planned.id }?.retention == .appDefault)
    }

    /// A route that already carries a desired level keeps it across an upload —
    /// the opt-in only fills a `nil`, it never overrides an explicit choice.
    @Test func anUploadKeepsAnExplicitLevel() async {
        let (model, control, library) = makeModel()
        // Seed the explicit level **before** the model loads the library.
        var planned = library.plannedRoutes().first { $0.deviceLink == nil }!
        planned.retention = .oneMonth
        library.savePlannedRoute(planned)

        model.start()
        await waitFor("the lists") { model.loadState == .loaded && model.connectedScope != nil }

        let payload = Data([4, 5, 6])
        let handle = MockTransport(control: control).uploadRoute(
            RouteBlob(summary: planned.summary, waypoints: planned.route.waypoints, payload: payload))
        guard await handle.outcome == .completed, let objectID = await handle.assignedObjectID else {
            Issue.record("mock upload must commit and assign an id"); return
        }
        model.markRouteUploaded(planned.id, objectID: objectID, crc32: CRC32.checksum(payload))

        await waitFor("the explicit-level push") {
            control.routeRetention(for: objectID) == .oneMonth
        }
        #expect(library.plannedRoutes().first { $0.id == planned.id }?.retention == .oneMonth)
    }
}
