import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The whole-trip upload queue driver (TR8, issue #657), host-side: the happy
/// path (stages then trip object), interrupt + resume (restart-current-stage),
/// the storage precheck (fails before any bytes), and the idempotent re-run.
/// Driven through `MainScreenModel.makeTripUploadModel` over the `trips` fixture,
/// exactly as `TripDetailView` wires it.
@MainActor
struct TripUploadModelTests {
    private let tripID = TripID("driftless-weekender")  // 2 fresh stages, no device copy

    private static let fastTiming = TripUploadModel.Timing(doneAutoDismiss: .milliseconds(40))

    private func makeMain(routesNearlyFull: Bool = false) async -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        control.loadFixtures("trips")
        control.routesNearlyFull = routesNearlyFull
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        await poll("first reconcile") { model.loadState == .loaded }
        return (model, control)
    }

    private func poll(
        _ what: String, timeout: Duration = .seconds(20), _ cond: @MainActor () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !cond() {
            #expect(ContinuousClock.now <= deadline, "timed out waiting for \(what)")
            if ContinuousClock.now > deadline { return }
            try? await Task.sleep(for: .milliseconds(5))
        }
    }

    /// Start the sheet and clear the `.ready` confirm. The happy-path device is
    /// retention-capable (epic #638), so the queue now holds on the Auto-delete
    /// confirm until the rider taps Upload; `beginUpload()` is a no-op on an
    /// incapable device (which starts running from `start()`).
    private func startAndConfirm(_ upload: TripUploadModel) {
        upload.start()
        upload.beginUpload()
    }

    // MARK: Happy path

    @Test
    func uploadsEveryStageThenTheTripObject() async {
        let (model, control) = await makeMain()
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        await poll("done") { upload.phase == .done }

        // Two fresh stages + the trip object committed; nothing skipped.
        #expect(upload.committedCount == 3)
        #expect(upload.skippedCount == 0)
        // The device now holds one trip referencing both stages.
        #expect(control.deviceTripCount == 1)
        let deviceTripID = control.deviceTripObjectIDs.first!
        #expect(control.deviceTripStageIDs(deviceTripID).count == 2)
        // The trip page reads fully up to date, and re-listing proves it.
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
    }

    // MARK: Interrupt + resume

    @Test
    func aDropInterruptsThenResumeFinishesTheTrip() async {
        let (model, control) = await makeMain()
        control.dropTransfer(atFraction: 0.5)  // one-shot: the first stage drops
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        await poll("interrupted") { upload.phase == .interrupted }

        upload.resume()
        await poll("done after resume") { upload.phase == .done }
        #expect(upload.committedCount == 3)
        #expect(control.deviceTripCount == 1)
    }

    // MARK: Storage precheck (fails before any bytes)

    @Test
    func storagePrecheckFailsUpfrontWithoutSendingBytes() async {
        let (model, control) = await makeMain(routesNearlyFull: true)
        // The device catalog is padded to one below the route cap; the trip has
        // two fresh stages, so it can't fit.
        let plan = try! #require(model.planTripUpload(tripID))
        #expect(!plan.precheck.fits)

        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        await poll("failed") { upload.phase == .failed }
        if case .storagePrecheck(let deficit) = upload.failure {
            #expect(deficit == 1)
        } else {
            Issue.record("expected a storage-precheck failure, got \(String(describing: upload.failure))")
        }
        // No bytes flowed — the device holds no trip and gained no route.
        #expect(control.deviceTripCount == 0)
    }

    // MARK: Idempotent re-run

    @Test
    func reRunningALandedTripSkipsEverything() async {
        let (model, control) = await makeMain()
        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(first)
        await poll("first done") { first.phase == .done }
        #expect(control.deviceTripCount == 1)

        // Re-run: every stage is up to date + the trip proven, so nothing is sent.
        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(second)
        await poll("second done") { second.phase == .done }
        #expect(second.committedCount == 0)
        #expect(second.skippedCount == 2)
        #expect(control.deviceTripCount == 1)  // no duplicate
    }

    // MARK: Auto-delete confirm (epic #638)

    /// A `makeMain` variant that also hands back the library so the retention
    /// tests can read the landed per-route records, and takes a scenario so the
    /// old-firmware (incapable) path is reachable.
    private func makeRetentionMain(_ scenario: Scenario = .happyPath)
        async -> (MainScreenModel, MockControl, any LibraryStore) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        await poll("first reconcile") { model.loadState == .loaded }
        return (model, control, library)
    }

    /// A retention-capable device seeds the app default and holds on the `.ready`
    /// confirm — `start()` does **not** begin the queue (the level is chosen first).
    @Test
    func capableTripSeedsTheDefaultAndHoldsOnReady() async {
        let (model, _, _) = await makeRetentionMain()
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        #expect(upload.supportsRetention)
        #expect(upload.phase == .ready)
        #expect(upload.retention == .twoWeeks)   // the documented app default

        upload.start()
        try? await Task.sleep(for: .milliseconds(60))
        #expect(upload.phase == .ready)          // still waiting on Upload trip
        #expect(upload.progress.bytesDone == 0)  // no bytes before the confirm
    }

    /// The confirm flow: `.ready` → begin → `.uploading` → `.done`, the queue
    /// intact behind the added gate.
    @Test
    func readyConfirmBeginsTheQueueAndFinishes() async {
        let (model, control, _) = await makeRetentionMain()
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        #expect(upload.phase == .ready)
        upload.start()
        upload.beginUpload()
        await poll("uploading") { upload.phase == .uploading || upload.phase == .done }
        await poll("done") { upload.phase == .done }
        #expect(upload.committedCount == 3)
        #expect(control.deviceTripCount == 1)
    }

    /// An old-firmware device (no expiry) skips the confirm entirely: the model
    /// opens on `.uploading` and `start()` runs the queue — the prior behaviour,
    /// the row never shown.
    @Test
    func incapableDeviceSkipsTheReadyConfirm() async {
        let (model, control, _) = await makeRetentionMain(.oldFirmware)
        await poll("capability settles") { !model.supportsRetention }
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        #expect(!upload.supportsRetention)
        #expect(upload.phase == .uploading)      // no confirm gate
        upload.start()
        await poll("done") { upload.phase == .done }
        #expect(upload.committedCount == 3)
        #expect(control.deviceTripCount == 1)
    }

    /// The crux (locked design): the trip's chosen Auto-delete level applies to
    /// **every** member route, overriding a member's own prior choice — a trip is
    /// one unit. One stage is pre-set to `.oneDay`; the rider picks `.twoMonths`
    /// for the trip; both stages land `.twoMonths`, on the device and in the library.
    @Test
    func tripRetentionOverridesEveryMemberRoutesOwnLevel() async {
        let (model, control, library) = await makeRetentionMain()
        let stageIDs = model.tripStages(tripID).map(\.id)
        #expect(stageIDs.count == 2)
        // Give one member its own, different level first.
        model.setRouteRetention(stageIDs[0], .oneDay)

        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        upload.start()
        upload.selectRetention(.twoMonths)       // the whole-trip pick
        #expect(upload.retention == .twoMonths)
        upload.beginUpload()
        await poll("done") { upload.phase == .done }

        // Every stage landed the trip's level — the per-route .oneDay is overridden.
        for id in stageIDs {
            let record = library.plannedRoutes().first { $0.id == id }
            let objectID = try! #require(record?.deviceLink?.objectID)
            #expect(control.routeRetention(for: objectID) == .twoMonths)
            #expect(record?.retention == .twoMonths)
        }
    }
}
