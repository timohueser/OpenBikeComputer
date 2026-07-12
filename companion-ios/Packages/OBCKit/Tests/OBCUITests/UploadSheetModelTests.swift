import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B5 acceptance, host-side: the upload-sheet model driven through
/// `MockTransport` — moving progress to F₂, cancel, the drop → interrupted →
/// restart path (uploads restart, not resume), and the hard-failure branch.
@MainActor
final class UploadSheetModelTests: XCTestCase {
    /// Instant F₂ auto-dismiss so tests don't sit out the design hold.
    private static let fastTiming = UploadSheetModel.Timing(doneAutoDismiss: .milliseconds(40))

    private func makeModel(
        _ scenario: Scenario,
        payloadBytes: Int = 100_000,
        waypoints: [Waypoint] = [],
        onCompleted: @escaping (DeviceObjectID?, UInt32) -> Void = { _, _ in }
    ) -> (UploadSheetModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast enough for test time, slow enough for several progress ticks (uploads
        // pace over a design-scale ~2 MB, so keep the throughput high).
        control.throughputBytesPerSec = 40_000_000
        let blob = RouteBlob(
            summary: RouteSummary(
                id: RouteID("upload-test"), name: "Kettle Moraine Loop",
                distanceMeters: 62_400, elevationGainMeters: 840
            ),
            waypoints: waypoints,
            payload: Data(count: payloadBytes)
        )
        let model = UploadSheetModel(
            transport: MockTransport(control: control),
            blob: blob,
            deviceName: "Trailhead",
            timing: Self.fastTiming,
            onCompleted: onCompleted
        )
        return (model, control)
    }

    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                XCTFail("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    // MARK: Happy path (F → F₂ → dismiss)

    func testHappyPathMovesThroughDoneAndAutoDismisses() async {
        var assignedObjectID: DeviceObjectID??
        let (model, _) = makeModel(.happyPath, onCompleted: { id, _ in assignedObjectID = id })

        XCTAssertEqual(model.phase, .uploading)
        XCTAssertEqual(model.fraction, 0)
        model.start()

        await waitFor("progress movement") { model.progress.bytesDone > 0 }
        // The mock paces uploads over a design-scale size (≈37 B/m), so the total
        // reflects the paced transfer, not the tiny real OBCR payload.
        XCTAssertGreaterThan(model.progress.total, 0)
        XCTAssertLessThanOrEqual(model.progress.bytesDone, model.progress.total)
        XCTAssertEqual(model.phase, .uploading)

        await waitFor("F₂") { model.phase == .done }
        XCTAssertNotNil(assignedObjectID, "onCompleted must fire on .completed")
        XCTAssertNotNil(assignedObjectID ?? nil, "the mock reports the device-assigned object id")
        XCTAssertEqual(model.fraction, 1)

        await waitFor("auto-dismiss") { model.shouldDismiss }
    }

    func testDerivedLinesMatchTheDesignReadout() {
        let (model, _) = makeModel(
            .happyPath,
            payloadBytes: 2_300_000,
            waypoints: [Waypoint(
                index: 0, name: "Ottawa Lake trailhead",
                distanceAlongMeters: 0, coordinate: Coordinate(latitude: 43, longitude: -88)
            )]
        )
        XCTAssertEqual(model.percentLine, "0%")
        // Numbers stay locale-aware ("0,0 / 2,3" on a German phone) — pin the
        // wiring against the formatter; OBCFormatTests pins the en-US string.
        XCTAssertEqual(
            model.sizeLine,
            OBCFormat.transferSizeLine(bytesDone: 0, totalBytes: 2_300_000, hasWaypoints: true)
        )
        XCTAssertEqual(model.routeName, "Kettle Moraine Loop")
        XCTAssertEqual(model.deviceName, "Trailhead")
    }

    // MARK: Cancel

    func testCancelResolvesCanceledAndDismisses() async {
        let (model, _) = makeModel(.happyPath, payloadBytes: 10_000_000)
        model.start()
        await waitFor("progress movement") { model.progress.bytesDone > 0 }

        model.cancel()
        await waitFor("dismiss after cancel") { model.shouldDismiss }
        XCTAssertNotEqual(model.phase, .done, "a cancel must never read as success")
        XCTAssertLessThan(model.progress.bytesDone, model.progress.total)
    }

    // MARK: Drop → interrupted → restart (uploadDrop scenario)

    func testDropInterruptsAndResumeRestartsFromScratch() async {
        let (model, _) = makeModel(.uploadDrop, payloadBytes: 100_000)
        model.start()

        // The armed drop (62%) parks the transfer and flags the link.
        await waitFor("interrupted") { model.phase == .interrupted }
        let stallBytes = model.progress.bytesDone
        XCTAssertGreaterThan(stallBytes, 0)
        XCTAssertLessThan(stallBytes, model.progress.total)
        XCTAssertFalse(model.shouldDismiss, "a drop is not terminal")

        // Nothing moves while parked.
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.progress.bytesDone, stallBytes)

        model.resume()
        XCTAssertEqual(model.phase, .uploading)
        // Restart, not resume: the whole object is re-sent (the device discarded
        // its partial), so the bar starts over and still reaches F₂.
        await waitFor("completion after restart") { model.phase == .done }
        XCTAssertEqual(model.fraction, 1)
    }

    func testCancelWhileInterruptedDismisses() async {
        let (model, _) = makeModel(.uploadDrop)
        model.start()
        await waitFor("interrupted") { model.phase == .interrupted }

        model.cancel()
        await waitFor("dismiss after cancel") { model.shouldDismiss }
        XCTAssertNotEqual(model.phase, .done)
    }

    /// A link that drops **straight to `.disconnected`** (never routing through
    /// `.outOfRange`) must still park the sheet in `.interrupted` — the same drop
    /// the sync watch reacts to. Without treating `.disconnected` as a drop the
    /// sheet wedges in `.uploading` with no Resume.
    func testDisconnectedMidUploadInterrupts() async {
        let (model, control) = makeModel(.happyPath, payloadBytes: 100_000)
        // Pace the upload glacially so the transfer can't complete (or tick)
        // before the drop lands — the test is about the drop, nothing else.
        control.throughputBytesPerSec = 1_000
        model.start()

        control.connection = .disconnected
        await waitFor("interrupted on .disconnected") { model.phase == .interrupted }
        XCTAssertFalse(model.shouldDismiss, "a drop is not terminal")

        model.sheetDismissed()
    }

    // MARK: Completion racing the dismiss

    /// The completion↔dismiss race: the transfer resolves `.completed` and the
    /// sheet is dismissed in the *same* turn. `sheetDismissed()` sees the resolved
    /// handle (so it leaves it alone) and cancels the watchers; the outcome
    /// watcher's `await` then returns immediately. It must **not** run the
    /// `.completed` branch on the torn-down sheet — no `onCompleted`, no
    /// resurrected `shouldDismiss`.
    func testCompletionRacingDismissDoesNotFireOnCompleted() async {
        let transport = ControlledUploadTransport()
        var completedCalls = 0
        let blob = RouteBlob(
            summary: RouteSummary(
                id: RouteID("race-test"), name: "Race", distanceMeters: 1_000,
                elevationGainMeters: 10
            ),
            waypoints: [],
            payload: Data(count: 1_000)
        )
        let model = UploadSheetModel(
            transport: transport,
            blob: blob,
            deviceName: "Trailhead",
            timing: Self.fastTiming,
            onCompleted: { _, _ in completedCalls += 1 }
        )
        transport.assignedID.fulfill(DeviceObjectID(7))
        model.start()

        // Let the outcome watcher reach its `await handle.outcome` suspension.
        try? await Task.sleep(for: .milliseconds(20))

        // Resolve + dismiss in one synchronous turn: fulfilling only *schedules*
        // the watcher's resume, so `sheetDismissed()` cancels it first.
        transport.outcomePromise.fulfill(.completed)
        model.sheetDismissed()

        // Give the cancelled watcher its chance to (not) act.
        try? await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(completedCalls, 0, "onCompleted must not fire after dismiss")
        XCTAssertNotEqual(model.phase, .done, "a raced completion must not resurrect the sheet")
        XCTAssertFalse(model.shouldDismiss)
    }

    // MARK: Hard failure (H4 — no link at all)

    func testUploadWithLinkDownFails() async {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        model.start()

        await waitFor("failed") { model.phase == .failed }
        XCTAssertFalse(model.shouldDismiss, "failure holds the sheet for the Close action")
        model.dismiss()
        XCTAssertTrue(model.shouldDismiss)
    }

    // MARK: Storage-full reject copy (L2 / #460)

    /// Build a model over a transport we drive straight to a chosen failure, so
    /// the copy mapping can be asserted without a scenario for each reject kind.
    private func failedModel(_ error: DeviceError) async -> UploadSheetModel {
        let transport = ControlledUploadTransport()
        let blob = RouteBlob(
            summary: RouteSummary(
                id: RouteID("fail-copy"), name: "Kettle Moraine Loop",
                distanceMeters: 62_400, elevationGainMeters: 840
            ),
            waypoints: [],
            payload: Data(count: 1_000)
        )
        let model = UploadSheetModel(
            transport: transport, blob: blob, deviceName: "Trailhead", timing: Self.fastTiming
        )
        model.start()
        try? await Task.sleep(for: .milliseconds(20))  // let the outcome watcher suspend
        transport.outcomePromise.fulfill(.failed(error))
        await waitFor("failed") { model.phase == .failed }
        return model
    }

    func testStorageFullFailureGetsDedicatedCopy() async {
        let model = await failedModel(.storageFull)
        XCTAssertEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Device storage full")
        XCTAssertEqual(
            model.failedMessage,
            "Trailhead's route storage is full. Delete routes on the device to make room, then try again."
        )
        // The copy must not imply an *update* of an existing route hits the cap.
        XCTAssertFalse(model.failedMessage.lowercased().contains("update"))
    }

    func testGenericRejectKeepsTheDefaultCopy() async {
        // A non-storage reject — including the forward-compat generic
        // `.transferRejected` an unknown status code decodes to — keeps the
        // "didn't answer" framing, byte-for-byte unchanged.
        let model = await failedModel(.transferRejected)
        XCTAssertEqual(model.failure, .transferRejected)
        XCTAssertNotEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Couldn't upload")
        XCTAssertEqual(
            model.failedMessage,
            "Trailhead didn't answer. Check that it's awake and nearby, then try again."
        )
    }
}

/// A hand-driven transport whose upload handle the test controls: the outcome
/// (and device-id) promises are held here so the completion↔dismiss race can be
/// sequenced deterministically, which the timing-driven `MockTransport` can't do.
/// Only `state` + `uploadRoute` are exercised; the rest is inert.
private final class ControlledUploadTransport: DeviceTransport, @unchecked Sendable {
    let outcomePromise = AsyncPromise<TransferOutcome>()
    let assignedID = AsyncPromise<DeviceObjectID?>()
    private let stateMulticast = AsyncMulticast<ConnectionState>(.connected)
    private let batteryMulticast = AsyncMulticast<Int>(100)
    private let finishedProgress: AsyncStream<TransferProgress>

    init() {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        continuation.finish()
        finishedProgress = stream
    }

    var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
    var battery: AsyncStream<Int> { batteryMulticast.stream() }
    var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }

    func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        TransferHandle(
            progress: finishedProgress,
            outcome: outcomePromise,
            assignedObjectID: assignedID,
            onCancel: { [outcomePromise] in outcomePromise.fulfill(.canceled) },
            onResume: {}
        )
    }

    // Unreachable in the upload-sheet tests.
    func connect() async throws {}
    func disconnect() async {}
    func deviceInfo() async throws -> DeviceInfo { fatalError("unused") }
    func readConfig() async throws -> DeviceConfig { fatalError("unused") }
    func writeConfig(_ config: DeviceConfig) async throws {}
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { fatalError("unused") }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> [RideSummary] { [] }
    func rideDetail(_ id: RideID) async throws -> RideDetail { fatalError("unused") }
    func downloadRides(_ ids: [RideID]) -> RideDownload { fatalError("unused") }
    func readDiagnostics() async throws -> Data { Data() }
}
