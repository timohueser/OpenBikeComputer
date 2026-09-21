import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The whole-trip upload queue driver: the happy path (stages, then the trip object), interrupt
/// and resume (which restarts the current stage), the flat-catalog against menu-cap boundary, and
/// the idempotent re-run. Driven through `MainScreenModel.makeTripUploadModel`.
@MainActor
struct TripUploadModelTests {
    private let tripID = TripID("driftless-weekender")  // 2 fresh stages, no device copy

    private static let fastTiming = TripUploadModel.Timing(doneAutoDismiss: .milliseconds(40))

    private func makeMain(routesNearlyFull: Bool = false) async throws -> (MainScreenModel, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        control.loadFixtures("trips")
        control.routesNearlyFull = routesNearlyFull
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        try await waitFor("first reconcile", timeout: .seconds(20), interval: .milliseconds(5)) { model.loadState == .loaded }
        return (model, control)
    }
    private func startAndConfirm(_ upload: TripUploadModel) {
        upload.start()
    }

    // MARK: Happy path

    @Test
    func uploadsEveryStageThenTheTripObject() async throws {
        let (model, control) = try await makeMain()
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        try await waitFor("done", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }

        // Two fresh stages and the trip object committed; nothing skipped.
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
    func aDropInterruptsThenResumeFinishesTheTrip() async throws {
        let (model, control) = try await makeMain()
        control.dropTransfer(atFraction: 0.5)  // one-shot: the first stage drops
        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        try await waitFor("interrupted", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .interrupted }

        upload.resume()
        try await waitFor("done after resume", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(upload.committedCount == 3)
        #expect(control.deviceTripCount == 1)
    }

    // MARK: Flat catalog vs. resident menu capacity

    @Test
    func aFullResidentRouteMenuDoesNotPretendTheFlatStoreIsFull() async throws {
        let (model, control) = try await makeMain(routesNearlyFull: true)
        // The mock catalog is padded to 63 routes. The device keeps at most 64 routes resident
        // for its menu, but that is not an admission cap: the flat store holds 1,916 entries.
        let plan = try! #require(model.planTripUpload(tripID))
        #expect(plan.precheck.fits)

        let upload = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(upload)
        try await waitFor("done", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(upload.committedCount == 3)
        #expect(control.deviceTripCount == 1)
    }

    // MARK: Idempotent re-run

    @Test
    func reRunningALandedTripSkipsEverything() async throws {
        let (model, control) = try await makeMain()
        let first = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(first)
        try await waitFor("first done", timeout: .seconds(20), interval: .milliseconds(5)) { first.phase == .done }
        #expect(control.deviceTripCount == 1)

        // Re-run: every stage is up to date and the trip is proven, so nothing is sent.
        let second = try! #require(model.makeTripUploadModel(tripID, timing: Self.fastTiming))
        startAndConfirm(second)
        try await waitFor("second done", timeout: .seconds(20), interval: .milliseconds(5)) { second.phase == .done }
        #expect(second.committedCount == 0)
        #expect(second.skippedCount == 2)
        #expect(control.deviceTripCount == 1)  // no duplicate
    }

}
