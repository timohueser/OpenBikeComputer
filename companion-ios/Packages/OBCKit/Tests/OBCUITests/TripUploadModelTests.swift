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

    /// The device-side half of a retention postcondition, waited for rather than
    /// asserted into the gap.
    ///
    /// The sheet's `.done` does **not** promise the device has the level yet:
    /// `MainScreenModel.pushRetention` fires the `setRouteRetention` write off in
    /// a detached task and ignores its result on purpose — the record is updated
    /// optimistically and "a failed send self-heals at the next reconcile". So
    /// `.done` orders the *library* half (see the `record?.retention` assertions,
    /// which are synchronous) but not this one.
    ///
    /// What finding #876-4 asks for is that the level **lands**, not that it
    /// lands synchronously with the sheet — so waiting for it tests the real
    /// postcondition. Asserting immediately tested the scheduler, and failed
    /// under load with the previous level still on the device.
    private func pollDeviceRetention(
        _ control: MockControl, _ objectID: DeviceObjectID, _ expected: Retention,
        _ comment: Comment? = nil
    ) async {
        await poll("device retention \(expected) on \(objectID)") {
            control.routeRetention(for: objectID) == expected
        }
        #expect(control.routeRetention(for: objectID) == expected, comment)
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
            await pollDeviceRetention(control, objectID, .twoMonths)
            #expect(record?.retention == .twoMonths)
        }
    }

    // MARK: Whole-trip retention reaches skipped stages (finding #876-4)

    /// The crux of finding #876-4: a re-run where **every** stage's bytes are already
    /// current (all skips, nothing transferred) still applies the trip's newly-chosen
    /// Auto-delete level to every member route — on the device and in the library. A
    /// skip skips the bytes, not the retention postcondition.
    @Test
    func reRunAtADifferentLevelUpdatesEverySkippedStage() async {
        let (model, control, library) = await makeRetentionMain()
        let stageIDs = model.tripStages(tripID).map(\.id)

        // First upload lands both stages fresh at the app default (.twoWeeks).
        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(first)
        await poll("first done") { first.phase == .done }
        for id in stageIDs {
            let objectID = try! #require(library.plannedRoutes().first { $0.id == id }?.deviceLink?.objectID)
            await pollDeviceRetention(control, objectID, .twoWeeks)
        }

        // Re-run: everything is current → an all-skip queue, no payload bytes. Pick a
        // *different* level for the whole trip.
        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        second.start()
        second.selectRetention(.twoMonths)
        second.beginUpload()
        await poll("second done") { second.phase == .done }
        #expect(second.committedCount == 0, "no route/trip payload bytes transfer")
        #expect(second.skippedCount == 2)

        // …yet every member route — all skipped — now carries the trip's new level.
        for id in stageIDs {
            let record = library.plannedRoutes().first { $0.id == id }
            let objectID = try! #require(record?.deviceLink?.objectID)
            await pollDeviceRetention(
                control, objectID, .twoMonths, "a skipped stage still got the trip's level")
            #expect(record?.retention == .twoMonths)
        }
    }

    /// A mix of a **fresh/replaced** stage and a **skipped** stage: both end at the
    /// selected trip level (finding #876-4). The device forgets one stage after the
    /// first upload, so the re-plan is one fresh + one skip.
    @Test
    func aFreshAndASkippedStageBothLandTheTripLevel() async {
        let (model, control, library) = await makeRetentionMain()
        let stageIDs = model.tripStages(tripID).map(\.id)

        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(first)
        await poll("first done") { first.phase == .done }

        // The device forgets stage 0 → its re-plan is fresh; stage 1 stays a skip.
        let goneObjectID = try! #require(library.plannedRoutes().first { $0.id == stageIDs[0] }?.deviceLink?.objectID)
        control.deviceDeletesRoute(goneObjectID)

        // Re-plan against the fresh catalog (prepareTripUpload re-reads + reconciles).
        let second = try! #require(await model.prepareTripUpload(tripID, timing: Self.fastTiming))
        second.start()
        second.selectRetention(.oneMonth)
        second.beginUpload()
        await poll("second done") { second.phase == .done }
        #expect(second.committedCount >= 1, "the forgotten stage re-uploads")
        #expect(second.skippedCount >= 1, "the surviving stage skips its bytes")

        for id in stageIDs {
            let record = library.plannedRoutes().first { $0.id == id }
            let objectID = try! #require(record?.deviceLink?.objectID)
            await pollDeviceRetention(
                control, objectID, .oneMonth, "fresh and skipped stages both land the trip level")
            #expect(record?.retention == .oneMonth)
        }
    }

    /// Idempotence: re-running at the **already-current** level sends **no** retention
    /// command (finding #876-4) — a skipped stage whose device level already matches
    /// pushes nothing.
    @Test
    func reRunAtTheSameLevelSendsNoRedundantRetentionCommand() async {
        let (model, control, library) = await makeRetentionMain()
        let stageIDs = model.tripStages(tripID).map(\.id)

        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(first)  // lands both stages at the .twoWeeks default
        await poll("first done") { first.phase == .done }
        // Let the first run's pushes land before counting. They ride detached
        // tasks (see `pollDeviceRetention`), so a snapshot taken at `.done` can
        // miss one and then read it as the second run's redundant write.
        for id in stageIDs {
            let objectID = try! #require(library.plannedRoutes().first { $0.id == id }?.deviceLink?.objectID)
            await pollDeviceRetention(control, objectID, .twoWeeks)
        }
        let writesAfterFirst = control.routeRetentionWriteCount

        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        second.start()
        second.selectRetention(.twoWeeks)  // the same level the stages already hold
        second.beginUpload()
        await poll("second done") { second.phase == .done }
        #expect(second.skippedCount == 2)
        #expect(
            control.routeRetentionWriteCount == writesAfterFirst,
            "re-selecting the current level pushes nothing — idempotent")
    }

    /// Compatibility: an incapable (old-firmware) device shows no picker and its
    /// skipped stages send **no** retention command (finding #876-4) — the flow is
    /// unchanged, `applyStageRetention` gates the push on capability.
    @Test
    func incapableDeviceRerunSendsNoRetentionCommand() async {
        let (model, control, _) = await makeRetentionMain(.oldFirmware)
        await poll("capability settles") { !model.supportsRetention }

        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        #expect(!first.supportsRetention)
        first.start()  // no confirm gate — runs straight away
        await poll("first done") { first.phase == .done }

        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        second.start()
        await poll("second done") { second.phase == .done }
        #expect(second.skippedCount == 2)
        #expect(control.routeRetentionWriteCount == 0, "an incapable device never gets a retention command")
    }
}
