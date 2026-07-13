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

    // MARK: Happy path

    @Test
    func uploadsEveryStageThenTheTripObject() async {
        let (model, control) = await makeMain()
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        upload.start()
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
        upload.start()
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
        upload.start()
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
        first.start()
        await poll("first done") { first.phase == .done }
        #expect(control.deviceTripCount == 1)

        // Re-run: every stage is up to date + the trip proven, so nothing is sent.
        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        second.start()
        await poll("second done") { second.phase == .done }
        #expect(second.committedCount == 0)
        #expect(second.skippedCount == 2)
        #expect(control.deviceTripCount == 1)  // no duplicate
    }
}
