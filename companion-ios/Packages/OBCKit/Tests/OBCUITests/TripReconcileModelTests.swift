import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// TR8 reconcile transitions + the adoption rule, host-side. Driven through
/// `MainScreenModel` over the `trips` fixture, exactly as the composition root
/// wires it.
@MainActor
struct TripReconcileModelTests {
    private let tripID = TripID("driftless-weekender")
    private static let fastTiming = TripUploadModel.Timing(doneAutoDismiss: .milliseconds(20))

    private func makeMain() async -> (MainScreenModel, MockControl) {
        let (model, control, _) = await makeMainWithLibrary()
        return (model, control)
    }

    private func makeMainWithLibrary() async -> (MainScreenModel, MockControl, InMemoryLibraryStore) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        control.throughputBytesPerSec = 40_000_000
        control.loadFixtures("trips")
        let library = InMemoryLibraryStore()
        control.seedLibrary(into: library)
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        await poll("loaded") { model.loadState == .loaded }
        return (model, control, library)
    }

    /// Simulate the **lost commit ack**: the trip object landed on the device but
    /// the phone never saw the `transferResult` — its library trip carries no
    /// link, exactly as if `markTripUploaded` never ran.
    private func stripTripLink(_ library: InMemoryLibraryStore, _ model: MainScreenModel) {
        var trip = library.trips().first { $0.id == tripID }!
        trip.deviceLink = nil
        trip.uploadedCRC32 = nil
        library.saveTrip(trip)
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

    /// Upload the whole trip and wait for it to land — the shared setup for the
    /// on-device transitions.
    private func uploadTrip(_ model: MainScreenModel) async {
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        await poll("trip landed") { upload.phase == .done }
    }

    /// Land a fresh loose route in the library + on the device, returning its id.
    private func importAndUploadRoute(
        _ model: MainScreenModel, control: MockControl, name: String
    ) async -> RouteID {
        let id = RouteID("new-\(name)")
        let points = (0..<20).map { i in
            RoutePoint(coordinate: Coordinate(latitude: 43.0 + 0.001 * Double(i), longitude: -89.0), elevationMeters: 200)
        }
        let route = ImportedRoute(name: name, points: points, waypoints: [])
        let summary = RouteSummary(id: id, name: name, distanceMeters: 2000, elevationGainMeters: 40)
        model.addImportedRoute(PlannedRouteRecord(
            summary: summary, route: route, sourceFileName: "\(name).gpx", sourceFileData: Data()))
        return id
    }

    private func singleRouteUpload(
        _ model: MainScreenModel, control: MockControl, routeID: RouteID
    ) async {
        // Encode exactly as the production single-route path does — the record's
        // geometry + waypoints under its **library name** — so the committed CRC
        // matches `RouteObjectCodec.payloadCRC(for:)` and the badge reads current.
        let name = model.routes.first { $0.id == routeID }!.name
        let geometry = model.plannedGeometry(for: routeID)!
        let payload = RouteObjectCodec.encode(points: geometry.points, waypoints: geometry.waypoints, name: name)
        let blob = RouteBlob(
            summary: RouteSummary(id: routeID, name: name, distanceMeters: 2000, elevationGainMeters: 40),
            waypoints: [], payload: payload,
            targetObjectID: model.plannedDeviceObjectID(for: routeID))
        let handle = MockTransport(control: control).uploadRoute(blob)
        #expect(await handle.outcome == .completed)
        let objectID = await handle.assignedObjectID!
        model.markRouteUploaded(routeID, objectID: objectID, crc32: CRC32.checksum(payload))
    }

    // MARK: Adoption rule

    @Test
    func uploadingARouteFiledInAnOnDeviceTripPushesTheTrip() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        #expect(control.deviceTripStageIDs(deviceTripID).count == 2)

        // File a new route into the on-device trip (the trip goes outdated), then
        // upload just that route — the adoption rule pushes the updated trip.
        let newRoute = await importAndUploadRoute(model, control: control, name: "Coda")
        model.fileRoute(newRoute, into: .existing(tripID))
        await singleRouteUpload(model, control: control, routeID: newRoute)

        // The adoption push runs in the background — wait for the page to read up
        // to date again (the commit lands after the device records the copy).
        await poll("trip adopted the route") { model.tripOnDeviceState(tripID) == .upToDate }
        #expect(control.deviceTripStageIDs(deviceTripID).count == 3)
    }

    @Test
    func uploadingARouteFiledInAnOfflineTripLandsItStandalone() async {
        let (model, control) = await makeMain()
        // The driftless trip is NOT on the device. File a new route into it and
        // upload the route — no trip object is pushed (the route lands standalone).
        let newRoute = await importAndUploadRoute(model, control: control, name: "Loose")
        model.fileRoute(newRoute, into: .existing(tripID))
        await singleRouteUpload(model, control: control, routeID: newRoute)

        // Give any (erroneous) adoption push a beat, then assert none happened.
        try? await Task.sleep(for: .milliseconds(80))
        #expect(control.deviceTripCount == 0)
    }

    // MARK: Reconcile transitions

    @Test
    func aDeviceSideTripDeleteClearsTheLink() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
        let deviceTripID = control.deviceTripObjectIDs.first!

        // The device forgets the trip (a trip-only delete) and notifies — the
        // reconcile clears the link, so the badge drops.
        control.deviceDeletesTrip(deviceTripID)
        await poll("link cleared") { model.tripOnDeviceState(tripID) == .notOnDevice }
    }

    @Test
    func aDeviceSideCascadeDeleteClearsTripAndStageLinks() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        let stageA = RouteID("devils-lake-overnighter")
        #expect(model.onDeviceState(stageA) == .upToDate)
        let deviceTripID = control.deviceTripObjectIDs.first!

        // TR3 long-press: the device deletes the trip AND its member routes.
        // The cascade notifies TWO storeChanged edges (route, then trip), each
        // triggering a reload that cancels its predecessor — the stage link can
        // clear a beat before the trip reconcile lands, so poll both (asserting
        // the trip state right after the stage poll raced the second reload).
        control.deviceDeletesTripCascade(deviceTripID)
        await poll("stage link cleared") { model.onDeviceState(stageA) == .notOnDevice }
        await poll("trip link cleared") { model.tripOnDeviceState(tripID) == .notOnDevice }
    }

    /// The on-glass regression (2026-07-13): the device deletes ONE member route
    /// (Route overview hold-delete); re-running "Upload trip" must re-send the
    /// missing stage and **replace the existing trip object in place** — never
    /// mint a second device trip.
    @Test
    func reUploadAfterADeviceSideStageDeleteReplacesTheTripInPlace() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let stageA = RouteID("devils-lake-overnighter")
        let stageDeviceID = model.plannedDeviceObjectID(for: stageA)!

        // The device-side route delete → storeChanged(route) → the app's
        // reconcile drops the stage link (trip badge goes off).
        control.deviceDeletesRoute(stageDeviceID)
        await poll("stage link cleared") { model.onDeviceState(stageA) == .notOnDevice }
        #expect(model.tripOnDeviceState(tripID) != .upToDate)

        // The re-upload plan: the missing stage is fresh, the trip object is a
        // replace of the existing device trip — never a second trip.
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))
        #expect(plan.stages.first { $0.routeID == stageA }?.action == .fresh)

        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        await poll("re-upload landed") { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "the re-upload must not mint a second device trip")
        #expect(control.deviceTripStageIDs(deviceTripID).count == 2)
        await poll("trip back up to date") { model.tripOnDeviceState(tripID) == .upToDate }
    }

    /// The root cause behind the on-glass duplicate: a **transient `listTrips`
    /// failure** during a reload must not read as "the device stores zero trips".
    /// Before the fix it dropped every trip's device link, and the next "Upload
    /// trip" minted a second device trip instead of replacing in place.
    @Test
    func aFailedTripListReadKeepsTheLinkAndTheNextUploadStillReplaces() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let stageA = RouteID("devils-lake-overnighter")
        let stageDeviceID = model.plannedDeviceObjectID(for: stageA)!

        // The device deletes a member route; the reload this triggers reads
        // `routeList` fine but the `tripList` read fails (a flaky link mid-read).
        control.failNextTripList(.readFailed)
        control.deviceDeletesRoute(stageDeviceID)
        await poll("stage link cleared") { model.onDeviceState(stageA) == .notOnDevice }

        // The trip's link survived the failed read — the plan still replaces.
        #expect(model.trip(tripID)?.deviceLink != nil, "a failed tripList read must not drop the link")
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))

        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        await poll("re-upload landed") { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "a failed tripList read must never cause a duplicate trip")
    }

    // MARK: Lost-ack recovery (the on-glass duplicate-trip bug, round 2)

    /// The trip object committed on the device but the phone missed the ack
    /// (link died at the wrong moment). The next reconcile must **adopt** the
    /// on-device trip by content — the trip twin of the #770 route rule — so the
    /// badge lights and a later push replaces in place.
    @Test
    func aLostTripCommitAckHealsByAdoptionOnTheNextReconcile() async {
        let (model, control, library) = await makeMainWithLibrary()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!

        stripTripLink(library, model)
        model.reload()
        await poll("trip re-adopted") { model.trip(tripID)?.deviceLink?.objectID == deviceTripID }
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
        let plan = model.planTripUpload(tripID)!
        #expect(plan.tripObject == .replace(deviceTripID))
    }

    /// The user's exact retry path: upload "failed" (ack lost after the device
    /// committed), they tap **Upload trip** again. `prepareTripUpload` re-reads
    /// the catalogs first, the reconcile adopts what actually landed, and the
    /// retry converges — one device trip, never a same-name twin.
    @Test
    func retryAfterALostTripAckDoesNotMintADuplicate() async {
        let (model, control, library) = await makeMainWithLibrary()
        await uploadTrip(model)
        #expect(control.deviceTripCount == 1)

        stripTripLink(library, model)
        // No reload in between — the retry itself must plan against fresh truth.
        let upload = await model.prepareTripUpload(tripID, timing: Self.fastTiming)!
        upload.start()
        await poll("retry landed") { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "the retry must never mint a second device trip")
        #expect(model.tripOnDeviceState(tripID) == .upToDate)
    }

    /// The device-side backstop (spec §4.2 fresh-upload dedup): even a client
    /// that plans blind — a fresh trip push of identical bytes, no reconcile —
    /// converges on the stored copy. The device answers with the existing id and
    /// stores nothing new.
    @Test
    func aBlindFreshReUploadOfIdenticalBytesConvergesOnTheStoredTrip() async {
        let (model, control, library) = await makeMainWithLibrary()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!

        stripTripLink(library, model)
        model.reloadTrips()
        // The OLD path: plan straight off the (now amnesiac) library — the trip
        // object plans fresh. The device's dedup still converges it.
        let upload = model.makeTripUploadModel(tripID, timing: Self.fastTiming)!
        upload.start()
        await poll("blind retry landed") { upload.phase == .done }
        #expect(control.deviceTripCount == 1, "identical bytes must dedup onto the stored trip")
        #expect(model.trip(tripID)?.deviceLink?.objectID == deviceTripID,
            "the commit links back to the existing object id")
    }

    // MARK: Delete trip & routes while connected

    @Test
    func deleteTripAndRoutesWhileConnectedDeletesDeviceCopies() async {
        let (model, control) = await makeMain()
        await uploadTrip(model)
        let deviceTripID = control.deviceTripObjectIDs.first!
        let stageObjectIDs = control.deviceTripStageIDs(deviceTripID)
        #expect(stageObjectIDs.count == 2)

        model.deleteTripAndRoutes(tripID)

        // The device-side cascade: both member routes deleted, then the trip.
        await poll("device cascade landed") {
            control.deletedTripObjectIDs.contains(deviceTripID)
                && stageObjectIDs.allSatisfy { control.deletedRouteObjectIDs.contains($0) }
        }
        // Phone library cleaned up too — the trip and its routes are gone.
        #expect(model.trip(tripID) == nil)
        #expect(model.routes.first { $0.id == RouteID("devils-lake-overnighter") } == nil)
    }
}
