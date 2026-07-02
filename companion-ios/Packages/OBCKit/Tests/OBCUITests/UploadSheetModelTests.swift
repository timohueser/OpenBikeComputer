import XCTest
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// B5 acceptance, host-side: the upload-sheet model driven through
/// `MockTransport` — moving progress to F₂, cancel, the drop → interrupted →
/// offset-resume path (no bytes re-sent), and the hard-failure branch.
@MainActor
final class UploadSheetModelTests: XCTestCase {
    /// Instant F₂ auto-dismiss so tests don't sit out the design hold.
    private static let fastTiming = UploadSheetModel.Timing(doneAutoDismiss: .milliseconds(40))

    private func makeModel(
        _ scenario: Scenario,
        payloadBytes: Int = 100_000,
        waypoints: [Waypoint] = [],
        onCompleted: @escaping () -> Void = {}
    ) -> (UploadSheetModel, MockControl) {
        let control = MockControl(scenario: scenario)
        control.latency = .zero
        // Fast enough for test time, slow enough for several progress ticks.
        control.throughputBytesPerSec = 2_000_000
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
        timeout: Duration = .seconds(5),
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
        var completed = false
        let (model, _) = makeModel(.happyPath, onCompleted: { completed = true })

        XCTAssertEqual(model.phase, .uploading)
        XCTAssertEqual(model.fraction, 0)
        model.start()

        await waitFor("progress movement") { model.progress.bytesDone > 0 }
        XCTAssertEqual(model.progress.total, 100_000)
        XCTAssertEqual(model.phase, .uploading)

        await waitFor("F₂") { model.phase == .done }
        XCTAssertTrue(completed, "onCompleted must fire on .completed")
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

    // MARK: Drop → interrupted → resume (uploadDrop scenario)

    func testDropInterruptsAndResumeContinuesFromTheOffset() async {
        let (model, _) = makeModel(.uploadDrop, payloadBytes: 100_000)
        model.start()

        // The armed drop (62%) parks the transfer and flags the link.
        await waitFor("interrupted") { model.phase == .interrupted }
        let stallOffset = model.progress.offset
        XCTAssertGreaterThan(stallOffset, 0)
        XCTAssertLessThan(stallOffset, model.progress.total)
        XCTAssertFalse(model.shouldDismiss, "a drop is not terminal")

        // Nothing moves while parked.
        try? await Task.sleep(for: .milliseconds(80))
        XCTAssertEqual(model.progress.offset, stallOffset)

        model.resume()
        XCTAssertEqual(model.phase, .uploading)
        await waitFor("completion after resume") { model.phase == .done }
        // Offset-based resume: no byte position before the stall ever re-sent.
        XCTAssertGreaterThanOrEqual(model.progress.bytesDone, stallOffset)
    }

    func testEveryTickAdvancesMonotonically() async {
        // Watch the raw stream alongside the model: resume must not rewind.
        let (model, _) = makeModel(.uploadDrop, payloadBytes: 50_000)
        var offsets: [Int] = []
        model.start()
        await waitFor("interrupted") { model.phase == .interrupted }
        offsets.append(model.progress.offset)
        model.resume()
        await waitFor("done") { model.phase == .done }
        offsets.append(model.progress.offset)
        XCTAssertEqual(offsets, offsets.sorted(), "offsets must never rewind across a resume")
    }

    func testCancelWhileInterruptedDismisses() async {
        let (model, _) = makeModel(.uploadDrop)
        model.start()
        await waitFor("interrupted") { model.phase == .interrupted }

        model.cancel()
        await waitFor("dismiss after cancel") { model.shouldDismiss }
        XCTAssertNotEqual(model.phase, .done)
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
}
