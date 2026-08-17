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
        onCompleted: @escaping (DeviceObjectID?, UInt32, Retention) -> Void = { _, _, _ in }
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
            // These exercise the transfer machinery — no retention capability, so
            // `start()` begins the transfer straight away (the `.ready` confirm is
            // covered by `UploadSheetRetentionTests`).
            supportsRetention: false,
            timing: Self.fastTiming,
            onCompleted: onCompleted
        )
        return (model, control)
    }

    /// Poll until `condition` holds, or **throw**. Recording a failure and
    /// returning would let the test fall through into assertions that were never
    /// going to hold, turning one timeout into a cascade of downstream failures
    /// that hide which wait actually blew (the shape this suite failed in CI).
    ///
    /// The deadline is generous for the reason spelled out in
    /// `WeatherSettingsModelTests`: Swift Testing schedules its suites
    /// concurrently in the same process as these `@MainActor` XCTest cases, so on
    /// a loaded runner a continuation can wait a long time for its turn on the
    /// main actor. The bound is here to catch a genuine hang, not to police
    /// latency. Prefer ``fulfillment(of:timeout:)`` on a signal the model itself
    /// fires for anything spanning a whole transfer — see the happy path.
    private func waitFor(
        _ what: String,
        timeout: Duration = .seconds(30),
        _ condition: () -> Bool
    ) async throws {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                throw WaitTimedOut(what: what, timeout: timeout)
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    // MARK: Happy path (F → F₂ → dismiss)

    func testHappyPathMovesThroughDoneAndAutoDismisses() async throws {
        // F₂ is *signalled*, not polled: `onCompleted` is the event under test, so
        // waiting on it directly can't race it. Polling could also only ever hurt
        // here — the mock paces a transfer in ~100 sequential hops that each need
        // the main actor, and a poll loop spinning on that same actor competes
        // with the very watcher it is waiting for.
        let completed = expectation(description: "onCompleted fires")
        var assignedObjectID: DeviceObjectID??
        let (model, _) = makeModel(.happyPath, onCompleted: { id, _, _ in
            assignedObjectID = id
            completed.fulfill()
        })

        XCTAssertEqual(model.phase, .uploading)
        XCTAssertEqual(model.fraction, 0)
        model.start()

        try await waitFor("progress movement") { model.progress.bytesDone > 0 }
        // The mock paces uploads over a design-scale size (≈37 B/m), so the total
        // reflects the paced transfer, not the tiny real OBCR payload.
        XCTAssertGreaterThan(model.progress.total, 0)
        XCTAssertLessThanOrEqual(model.progress.bytesDone, model.progress.total)
        XCTAssertEqual(model.phase, .uploading)

        // `fulfill()` only *schedules* this test's resume, so the watcher runs on
        // to `phase = .done` before its next suspension — the model's documented
        // "fires before `phase` reads `.done`" contract, observed from the side
        // that contract is written for.
        await fulfillment(of: [completed], timeout: 30)
        // Reaching here *is* "onCompleted fired on .completed" — the expectation
        // covers the outer optional, so what's left to check is the id it carried.
        XCTAssertNotNil(assignedObjectID ?? nil, "the mock reports the device-assigned object id")
        XCTAssertEqual(model.phase, .done, "F₂ is observable once the save has run")
        XCTAssertEqual(model.fraction, 1)

        try await waitFor("auto-dismiss") { model.shouldDismiss }
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

    func testCancelResolvesCanceledAndDismisses() async throws {
        let (model, _) = makeModel(.happyPath, payloadBytes: 10_000_000)
        model.start()
        try await waitFor("progress movement") { model.progress.bytesDone > 0 }

        model.cancel()
        try await waitFor("dismiss after cancel") { model.shouldDismiss }
        XCTAssertNotEqual(model.phase, .done, "a cancel must never read as success")
        XCTAssertLessThan(model.progress.bytesDone, model.progress.total)
    }

    // MARK: Drop → interrupted → restart (uploadDrop scenario)

    func testDropInterruptsAndResumeRestartsFromScratch() async throws {
        let (model, _) = makeModel(.uploadDrop, payloadBytes: 100_000)
        model.start()

        // The armed drop (62%) parks the transfer and flags the link.
        try await waitFor("interrupted") { model.phase == .interrupted }
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
        try await waitFor("completion after restart") { model.phase == .done }
        XCTAssertEqual(model.fraction, 1)
    }

    func testCancelWhileInterruptedDismisses() async throws {
        let (model, _) = makeModel(.uploadDrop)
        model.start()
        try await waitFor("interrupted") { model.phase == .interrupted }

        model.cancel()
        try await waitFor("dismiss after cancel") { model.shouldDismiss }
        XCTAssertNotEqual(model.phase, .done)
    }

    /// A link that drops **straight to `.disconnected`** (never routing through
    /// `.outOfRange`) must still park the sheet in `.interrupted` — the same drop
    /// the sync watch reacts to. Without treating `.disconnected` as a drop the
    /// sheet wedges in `.uploading` with no Resume.
    func testDisconnectedMidUploadInterrupts() async throws {
        let (model, control) = makeModel(.happyPath, payloadBytes: 100_000)
        // Pace the upload glacially so the transfer can't complete (or tick)
        // before the drop lands — the test is about the drop, nothing else.
        control.throughputBytesPerSec = 1_000
        model.start()

        control.connection = .disconnected
        try await waitFor("interrupted on .disconnected") { model.phase == .interrupted }
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
            supportsRetention: false,
            timing: Self.fastTiming,
            onCompleted: { _, _, _ in completedCalls += 1 }
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

    func testUploadWithLinkDownFails() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        model.start()

        try await waitFor("failed") { model.phase == .failed }
        XCTAssertFalse(model.shouldDismiss, "failure holds the sheet for the Close action")
        model.dismiss()
        XCTAssertTrue(model.shouldDismiss)
    }

    // MARK: Storage-full reject copy (L2 / #460)

    /// Build a model over a transport we drive straight to a chosen failure, so
    /// the copy mapping can be asserted without a scenario for each reject kind.
    private func failedModel(_ error: DeviceError) async throws -> UploadSheetModel {
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
            transport: transport, blob: blob, deviceName: "Trailhead",
            supportsRetention: false, timing: Self.fastTiming
        )
        model.start()
        try? await Task.sleep(for: .milliseconds(20))  // let the outcome watcher suspend
        transport.outcomePromise.fulfill(.failed(error))
        try await waitFor("failed") { model.phase == .failed }
        return model
    }

    func testStorageFullFailureGetsDedicatedCopy() async throws {
        let model = try await failedModel(.storageFull)
        XCTAssertEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Device storage full")
        XCTAssertEqual(
            model.failedMessage,
            "Trailhead's route storage is full. Delete routes on the device to make room, then try again."
        )
        // The copy must not imply an *update* of an existing route hits the cap.
        XCTAssertFalse(model.failedMessage.lowercased().contains("update"))
    }

    func testGenericRejectKeepsTheDefaultCopy() async throws {
        // A non-storage reject — including the forward-compat generic
        // `.transferRejected` an unknown status code decodes to — keeps the
        // "didn't answer" framing, byte-for-byte unchanged.
        let model = try await failedModel(.transferRejected)
        XCTAssertEqual(model.failure, .transferRejected)
        XCTAssertNotEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Couldn't upload")
        XCTAssertEqual(
            model.failedMessage,
            "Trailhead didn't answer. Check that it's awake and nearby, then try again."
        )
    }
}

/// A `waitFor` that gave up. Thrown rather than recorded, so the test that was
/// waiting stops instead of falling through into assertions it has already lost.
private struct WaitTimedOut: Error, CustomStringConvertible {
    let what: String
    let timeout: Duration

    var description: String {
        "timed out after \(timeout) waiting for \(what)"
    }
}

/// A hand-driven transport whose upload handle the test controls: the outcome
/// (and device-id) promises are held here so the completion↔dismiss race can be
/// sequenced deterministically, which the timing-driven `MockTransport` can't do.
/// Only `state` + `uploadRoute` are exercised; the rest is inert.
private final class ControlledUploadTransport: DeviceLink, DeviceObjects, @unchecked Sendable {
    let outcomePromise = AsyncPromise<TransferOutcome>()
    let assignedID = AsyncPromise<DeviceObjectID?>()
    private let stateMulticast = AsyncMulticast<ConnectionState>(.connected)
    private let finishedProgress: AsyncStream<TransferProgress>

    init() {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        continuation.finish()
        finishedProgress = stream
    }

    var state: AsyncStream<ConnectionState> { stateMulticast.stream() }

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
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { fatalError("unused") }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { RideCatalog(rides: []) }
    func rideDetail(_ id: RideID) async throws -> RideDetail { fatalError("unused") }
    func downloadRides(_ ids: [RideID]) -> RideDownload { fatalError("unused") }
}
