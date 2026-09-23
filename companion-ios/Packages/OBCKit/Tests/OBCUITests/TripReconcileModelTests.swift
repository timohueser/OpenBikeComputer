import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Trip reconcile transitions and re-uploads, driven through `MainScreenModel` over the
/// trips fixture, exactly as the composition root wires it.
@MainActor
struct TripReconcileModelTests {
    private let tripID = TripID("driftless-weekender")
    private static let fastTiming = TripUploadModel.Timing(doneAutoDismiss: .milliseconds(20))

    private func makeMain() async throws -> (MainScreenModel, MockControl) {
        let (model, control, _) = try await makeMainWithLibrary()
        return (model, control)
    }

    private func makeMainWithLibrary() async throws -> (MainScreenModel, MockControl, InMemoryLibraryStore) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        try await waitFor("loaded", timeout: .seconds(20), interval: .milliseconds(5)) { model.loadState == .loaded }
        return (model, control, library)
    }

    /// Simulate the lost commit ack: the trip object landed on the device but the phone never saw
    /// the final commit result, so its library trip carries no link.
    private func stripTripLink(_ library: InMemoryLibraryStore, _ model: MainScreenModel) {
        var trip = library.trips().first { $0.id == tripID }!
        trip.deviceLink = nil
        trip.uploadedCRC32 = nil
        library.saveTrip(trip)
    }

    /// Upload the whole trip and wait for it to land: the shared setup for the on-device
    /// transitions.
    private func uploadTrip(_ model: MainScreenModel) async throws {
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("trip landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
    }

    private func dayObjectID(_ model: MainScreenModel, _ day: Int) -> DeviceObjectID? {
        model.trip(tripID)?.dayCopies[day]?.link.objectID
    }

    // MARK: Re-cut

    /// A day added to an uploaded trip leaves the other days' bytes unchanged, so the re-upload
    /// skips them, sends the new day, and replaces the trip object in place.
    @Test
    func aReCutSkipsTheDaysWhoseBytesDidNotChange() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let file = (0..<20).map { i in
            RoutePoint(coordinate: Coordinate(latitude: 43.2 + 0.001 * Double(i), longitude: -89.6), elevationMeters: 300)
        }
        model.appendToTrip(tripID, file: file)
        #expect(model.tripOnDeviceState(tripID) == .outdated)

        let plan = model.planTripUpload(tripID)!
        #expect(plan.days.map(\.action) == [.skip, .skip, .fresh])
        #expect(plan.tripObject == .replace(deviceTripID))
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("re-cut landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripStageIDs(deviceTripID).count == 3)
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
    }

    /// A reverse changes every day route and the trip key: each day route and the trip object are
    /// replaced in place, and the device reads the new key.
    @Test
    func aReversedTripReplacesEveryObjectInPlace() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let dayIDs = [dayObjectID(model, 0)!, dayObjectID(model, 1)!]

        model.reverseTrip(tripID)
        let plan = model.planTripUpload(tripID)!
        #expect(plan.days.map(\.action) == dayIDs.map(TripDayAction.replace))
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("reverse landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1)
        #expect(control.deviceTripStageIDs(deviceTripID) == dayIDs)
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
    }

    // MARK: Reconcile transitions

    @Test
    func aDeviceSideTripDeleteClearsTheLink() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
        let deviceTripID = control.deviceTripObjectIDs.first!

        // The device forgets the trip and notifies, so the reconcile clears the link and the badge
        // drops.
        control.deviceDeletesTrip(deviceTripID)
        try await waitFor("link cleared", timeout: .seconds(20), interval: .milliseconds(5)) { model.tripOnDeviceState(tripID) == .notOnDevice }
    }

    @Test
    func aDeviceSideCascadeDeleteClearsTripAndDayLinks() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        #expect(dayObjectID(model, 0) != nil)
        let deviceTripID = control.deviceTripObjectIDs.first!

        // The device deletes the trip and its day routes. The cascade notifies two store-change
        // edges, route then trip, each triggering a reload that cancels its predecessor, so the
        // day link can clear a beat before the trip reconcile lands. Poll both.
        control.deviceDeletesTripCascade(deviceTripID)
        try await waitFor("day link cleared", timeout: .seconds(20), interval: .milliseconds(5)) { dayObjectID(model, 0) == nil }
        try await waitFor("trip link cleared", timeout: .seconds(20), interval: .milliseconds(5)) { model.tripOnDeviceState(tripID) == .notOnDevice }
    }

    /// The device deletes one day route. Re-running "Upload trip" must re-send the missing day
    /// and replace the existing trip object in place, never mint a second device trip.
    @Test
    func reUploadAfterADeviceSideDayDeleteReplacesTheTripInPlace() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let dayDeviceID = dayObjectID(model, 0)!

        // The device-side route delete notifies, and the app's reconcile drops the day link, so
        // the trip badge goes off.
        control.deviceDeletesRoute(dayDeviceID)
        try await waitFor("day link cleared", timeout: .seconds(20), interval: .milliseconds(5)) { dayObjectID(model, 0) == nil }
        #expect(model.tripOnDeviceState(tripID) != .upToDate)

        // The re-upload plan: the missing day is fresh, and the trip object is a replace of the
        // existing device trip, never a second one.
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))
        #expect(plan.days.map(\.action) == [.fresh, .skip])

        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("re-upload landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "the re-upload must not mint a second device trip")
        #expect(control.deviceTripStageIDs(deviceTripID).count == 2)
        try await waitFor("trip back up to date", timeout: .seconds(20), interval: .milliseconds(5)) { model.tripOnDeviceState(tripID) == .upToDate }
    }

    /// A transient `listTrips` failure during a reload must not read as "the device stores zero
    /// trips". Read that way it drops every trip's device link, and the next "Upload trip" mints a
    /// second device trip instead of replacing in place.
    @Test
    func aFailedTripCatalogReadKeepsTheLinkAndTheNextUploadStillReplaces() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let dayDeviceID = dayObjectID(model, 0)!

        // The device deletes a day route. In the reload this triggers, the route catalog
        // succeeds and the trip catalog fails, as a flaky link mid-read would.
        control.failNextTripCatalog(.readFailed)
        control.deviceDeletesRoute(dayDeviceID)
        try await waitFor("day link cleared", timeout: .seconds(20), interval: .milliseconds(5)) { dayObjectID(model, 0) == nil }

        // The trip's link survived the failed read, so the plan still replaces.
        #expect(model.trip(tripID)?.deviceLink != nil, "a failed trip catalog read must not drop the link")
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))

        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("re-upload landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "a failed trip catalog read must never cause a duplicate trip")
    }

    // MARK: Lost-ack recovery

    /// The trip object committed on the device but the phone missed the ack. The next reconcile
    /// must adopt the on-device trip by content, the trip twin of the route rule, so the badge
    /// lights and a later push replaces in place.
    @Test
    func aLostTripCommitAckHealsByAdoptionOnTheNextReconcile() async throws {
        let (model, control, library) = try await makeMainWithLibrary()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!

        stripTripLink(library, model)
        model.reload()
        try await waitFor("trip re-adopted", timeout: .seconds(20), interval: .milliseconds(5)) { model.trip(tripID)?.deviceLink?.objectID == deviceTripID }
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))
    }

    /// The rename twin of the case above: the trip name lives inside the trip object, so a trip
    /// renamed while unlinked no longer matches its device copy by content. The catalog reports the
    /// name that copy was stored under, which is what makes the stored bytes reconstructible;
    /// without it the app reads the trip as absent and the next send mints a twin.
    @Test
    func aRenamedTripAdoptsItsDeviceCopyAndReplacesIt() async throws {
        let (model, control, library) = try await makeMainWithLibrary()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!

        // Rename first, then drop the link: `renameTrip` saves the model's cached trip, so
        // stripping before it would only be written back.
        model.renameTrip(tripID, to: "Driftless, the long way")
        stripTripLink(library, model)
        model.reloadTrips()
        #expect(
            model.trip(tripID)?.deviceLink == nil,
            "precondition: the model reads the trip as unlinked before the reconcile")

        model.reload()
        try await waitFor("the renamed trip adopts its device copy", timeout: .seconds(20), interval: .milliseconds(5)) {
            model.trip(tripID)?.deviceLink?.objectID == deviceTripID
        }
        #expect(
            model.tripOnDeviceState(tripID) == .outdated,
            "the device holds it under the old name — on the device, out of date")
        #expect(model.planTripUpload(tripID)?.tripObject == .replace(deviceTripID))

        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("the renamed send landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "a renamed send replaces by id — never a duplicate")
        #expect(
            control.deviceTripObjectIDs.first == deviceTripID,
            "…and the device copy kept its id, so the send was a replace")
    }

    /// The rider's exact retry path: the upload "failed" because the ack was lost after the device
    /// committed, and they tap Upload trip again. `prepareTripUpload` re-reads the catalogs first,
    /// the reconcile adopts what actually landed, and the retry converges on one device trip.
    @Test
    func retryAfterALostTripAckDoesNotMintADuplicate() async throws {
        let (model, control, library) = try await makeMainWithLibrary()
        try await uploadTrip(model)
        #expect(control.deviceTripCount == 1)

        stripTripLink(library, model)
        // No reload in between: the retry itself must plan against fresh truth.
        let upload = await model.prepareTripUpload(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("retry landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "the retry must never mint a second device trip")
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
    }

    /// The device-side backstop: even a client that plans blind, with a fresh trip push of
    /// identical bytes and no reconcile, converges on the stored copy. The device answers with the
    /// existing id and stores nothing new.
    @Test
    func aBlindFreshReUploadOfIdenticalBytesConvergesOnTheStoredTrip() async throws {
        let (model, control, library) = try await makeMainWithLibrary()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!

        stripTripLink(library, model)
        model.reloadTrips()
        // Plan straight off the now amnesiac library, so the trip object plans fresh. The device's
        // dedup still converges it.
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        try await waitFor("blind retry landed", timeout: .seconds(20), interval: .milliseconds(5)) { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "identical bytes must dedup onto the stored trip")
        #expect(model.trip(tripID)?.deviceLink?.objectID == deviceTripID,
            "the commit links back to the existing object id")
    }

    // MARK: Delete while connected

    @Test
    func deleteTripWhileConnectedDeletesDeviceCopies() async throws {
        let (model, control) = try await makeMain()
        try await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let dayObjectIDs = control.deviceTripStageIDs(deviceTripID)
        #expect(dayObjectIDs.count == 2)

        model.deleteTrip(tripID)

        try await waitFor("device cascade landed", timeout: .seconds(20), interval: .milliseconds(5)) {
            control.deletedTripObjectIDs.contains(deviceTripID)
                && dayObjectIDs.allSatisfy { control.deletedRouteObjectIDs.contains($0) }
        }
        #expect(model.trip(tripID) == nil)
    }
}
