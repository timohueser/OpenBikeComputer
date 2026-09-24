import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// The upload-sheet model driven through `MockTransport`: progress, cancel, the drop to
/// interrupted to restart path (uploads restart, they do not resume), and hard failure.
@MainActor
final class UploadSheetModelTests: XCTestCase {
    /// A near-instant auto-dismiss so tests do not sit out the design hold.
    private static let fastTiming = UploadSheetModel.Timing(doneAutoDismiss: .milliseconds(40))

    private func makeModel(
        _ scenario: Scenario,
        payloadBytes: Int = 100_000,
        waypoints: [Waypoint] = [],
        onCompleted: @escaping (DeviceObjectID?, UInt32) -> Void = { _, _ in }
    ) -> (UploadSheetModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast enough for test time, slow enough for several progress ticks: the mock paces
        // over a design-scale payload, so the throughput must stay high.
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

    // MARK: Happy path

    func testHappyPathMovesThroughDoneAndAutoDismisses() async throws {
        // `onCompleted` is signalled, not polled: waiting on it directly cannot race it. A poll
        // loop would also compete for the main actor with the watcher it waits for.
        let completed = expectation(description: "onCompleted fires")
        var assignedObjectID: DeviceObjectID??
        let (model, _) = makeModel(.happyPath, onCompleted: { id, _ in
            assignedObjectID = id
            completed.fulfill()
        })

        XCTAssertEqual(model.phase, .uploading)
        XCTAssertEqual(model.fraction, 0)
        model.start()

        try await waitFor("progress movement") { model.progress.bytesDone > 0 }
        // The mock paces uploads over a design-scale size, so `total` reflects the paced
        // transfer, not the small test payload.
        XCTAssertGreaterThan(model.progress.total, 0)
        XCTAssertLessThanOrEqual(model.progress.bytesDone, model.progress.total)
        XCTAssertEqual(model.phase, .uploading)

        // `fulfill()` only schedules this test's resume, so the watcher runs on to
        // `phase = .done` before its next suspension.
        await fulfillment(of: [completed], timeout: 30)
        // The expectation covers the outer optional; what is left to check is the id it carried.
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
        // Assert against the formatter, not a literal: the numbers are locale-aware.
        // OBCFormatTests pins the en-US string.
        XCTAssertEqual(
            model.sizeLine,
            OBCFormat.transferSizeLine(bytesDone: 0, totalBytes: 2_300_000, hasWaypoints: true)
        )
        XCTAssertEqual(model.overview.name, "Kettle Moraine Loop")
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

        try await waitFor("interrupted") { model.phase == .interrupted }
        let stallBytes = model.progress.bytesDone
        XCTAssertGreaterThan(stallBytes, 0)
        XCTAssertLessThan(stallBytes, model.progress.total)
        XCTAssertFalse(model.shouldDismiss, "a drop is not terminal")

        let stayedParked = await neverHolds({
            model.progress.bytesDone != stallBytes || model.phase != .interrupted
        }, for: .milliseconds(80))
        XCTAssertTrue(stayedParked, "a parked transfer must not move or resume itself")
        XCTAssertEqual(model.progress.bytesDone, stallBytes)

        model.resume()
        XCTAssertEqual(model.phase, .uploading)
        // Restart, not resume: the whole object is re-sent, so the bar starts over.
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

    /// A link that drops straight to `.disconnected`, never routing through `.outOfRange`, must
    /// still park the sheet in `.interrupted`. Otherwise it wedges in `.uploading` with no Resume.
    func testDisconnectedMidUploadInterrupts() async throws {
        let (model, control) = makeModel(.happyPath, payloadBytes: 100_000)
        // Pace the upload glacially so it cannot complete or tick before the drop lands.
        control.throughputBytesPerSec = 1_000
        model.start()

        control.connection = .disconnected
        try await waitFor("interrupted on .disconnected") { model.phase == .interrupted }
        XCTAssertFalse(model.shouldDismiss, "a drop is not terminal")

        model.sheetDismissed()
    }

    // MARK: Completion racing the dismiss

    /// The completion and the dismiss land in the same turn: `sheetDismissed()` sees the resolved
    /// handle and cancels the watchers, so the outcome watcher's `await` returns at once. It must
    /// not run the `.completed` branch on a torn-down sheet.
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
        transport.outcomePromise.fulfill(.completed)
        model.start()

        // The handle is already resolved when `start()` schedules its watcher. Dismiss in the
        // same synchronous turn, before that watcher can act on the completion.
        model.sheetDismissed()

        let stayedDismissed = await neverHolds({
            completedCalls > 0 || model.phase == .done || model.shouldDismiss
        }, for: .milliseconds(30))
        XCTAssertTrue(stayedDismissed, "the canceled watcher must not act on the resolved handle")
        XCTAssertEqual(completedCalls, 0, "onCompleted must not fire after dismiss")
        XCTAssertNotEqual(model.phase, .done, "a raced completion must not resurrect the sheet")
        XCTAssertFalse(model.shouldDismiss)
    }

    // MARK: Hard failure (no link at all)

    func testUploadWithLinkDownFails() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        model.start()

        try await waitFor("failed") { model.phase == .failed }
        XCTAssertFalse(model.shouldDismiss, "failure holds the sheet for the Close action")
        model.dismiss()
        XCTAssertTrue(model.shouldDismiss)
    }

    /// Send again after a failure starts a new transfer, which completes once the link is back.
    func testRetryAfterFailureSends() async throws {
        let (model, control) = makeModel(.happyPath)
        control.connection = .disconnected
        model.start()
        try await waitFor("failed") { model.phase == .failed }

        control.connection = .connected
        model.retry()
        XCTAssertNil(model.failure)
        try await waitFor("done after retry") { model.phase == .done }
    }

    // MARK: Storage-full reject copy

    /// A model over a transport driven straight to a chosen failure, so the copy mapping can be
    /// asserted without a scenario per reject kind.
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
        )
        transport.outcomePromise.fulfill(.failed(error))
        model.start()
        try await waitFor("failed") { model.phase == .failed }
        return model
    }

    func testStorageFullFailureGetsDedicatedCopy() async throws {
        let model = try await failedModel(.storageFull)
        XCTAssertEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Trailhead is full")
        XCTAssertEqual(
            model.failedMessage,
            "There is no room for more routes. Delete routes on Trailhead, then send again."
        )
        // The copy must not imply that updating an existing route hits the cap.
        XCTAssertFalse(model.failedMessage.lowercased().contains("update"))
    }

    func testGenericRejectKeepsTheDefaultCopy() async throws {
        // An unknown status code decodes to the generic `.transferRejected`, which keeps the
        // default framing.
        let model = try await failedModel(.transferRejected)
        XCTAssertEqual(model.failure, .transferRejected)
        XCTAssertNotEqual(model.failure, .storageFull)
        XCTAssertEqual(model.failedTitle, "Not sent")
        XCTAssertEqual(
            model.failedMessage,
            "Trailhead did not answer. Make sure it is on and near your phone, then send again."
        )
    }
}

/// A hand-driven transport: the outcome and device-id promises are held here, so the completion
/// and dismiss race can be sequenced deterministically. Only `state` and `uploadRoute` are live.
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
    func downloadRides(_ ids: [RideID]) -> RideDownload { fatalError("unused") }
}
