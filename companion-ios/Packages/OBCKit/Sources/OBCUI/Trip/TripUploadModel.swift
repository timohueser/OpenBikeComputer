import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Drives a **whole-trip upload** (TR8, issue #657) — the queued sibling of
/// `UploadSheetModel`. One transfer in flight at a time, in ride order: each
/// stage is skipped (CRC up-to-date), replaced in place (on device but
/// outdated), or freshly uploaded (absent), then the **trip object last**. The
/// precheck runs before any bytes — a trip that can't fit fails upfront with the
/// "delete routes on the device" guidance, never `storageFull` at stage 4.
///
/// Interruption keeps `UploadSheetModel`'s restart-current-stage semantics
/// (uploads restart, not resume); completed stages stay committed; re-running is
/// idempotent (the skips catch everything already landed). Each object's link is
/// committed the instant its transfer lands (via the step's `commit` closure into
/// `MainScreenModel`), exactly like a single upload.
@MainActor @Observable
public final class TripUploadModel: Identifiable {
    public nonisolated let id = UUID()

    public enum Phase: Equatable, Sendable {
        case uploading
        case interrupted
        case done
        case failed
    }

    /// Why the whole-trip upload failed — a precheck deficit (before any bytes) or
    /// a device reject mid-queue. Drives the failure copy.
    public enum Failure: Equatable, Sendable {
        /// The precheck found the trip can't fit: `routeDeficit` route slots short.
        case storagePrecheck(routeDeficit: Int)
        /// A device transfer failed for good (storage-full at open, a reject, …).
        case device(DeviceError)
    }

    public struct Timing: Sendable {
        public var doneAutoDismiss: Duration
        public init(doneAutoDismiss: Duration = .seconds(2.6)) {
            self.doneAutoDismiss = doneAutoDismiss
        }
    }

    /// One queue step — a skipped stage (no bytes) or a transfer (a stage or the
    /// trip object). `makeTransfer` is evaluated **at execution time** so the trip
    /// object step reads the stage ids the just-committed stages landed under; it
    /// returns `nil` to degenerate to a skip (nothing resolvable to send).
    public struct QueueStep: Sendable {
        let title: String
        let skip: Bool
        let makeTransfer: (@MainActor @Sendable () -> (handle: TransferHandle, committedCRC: UInt32)?)?
        let commit: (@MainActor @Sendable (DeviceObjectID?, UInt32) -> Void)?

        /// A stage already up-to-date on the device — flashes by, tallied.
        public static func skip(title: String) -> QueueStep {
            QueueStep(title: title, skip: true, makeTransfer: nil, commit: nil)
        }

        /// A transfer step (a stage upload or the trip object).
        public static func transfer(
            title: String,
            makeTransfer: @escaping @MainActor @Sendable () -> (handle: TransferHandle, committedCRC: UInt32)?,
            commit: @escaping @MainActor @Sendable (DeviceObjectID?, UInt32) -> Void
        ) -> QueueStep {
            QueueStep(title: title, skip: false, makeTransfer: makeTransfer, commit: commit)
        }
    }

    // MARK: Observable state

    public private(set) var phase: Phase = .uploading
    public private(set) var progress = TransferProgress(bytesDone: 0, total: 1)
    public private(set) var failure: Failure?
    public private(set) var shouldDismiss = false
    /// The current queue step (0-based) — drives the "Stage X of Y" header.
    public private(set) var stepIndex = 0
    /// Stages skipped because the device already held them, current — the done
    /// state's tally.
    public private(set) var skippedCount = 0
    /// Objects committed (stages + trip) so far — the done state's tally.
    public private(set) var committedCount = 0

    // MARK: Fixed facts

    public let tripName: String
    public let deviceName: String
    /// Total queue steps (skips + uploads + trip object) — the "of Y" denominator.
    public let stepCount: Int

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let steps: [QueueStep]
    private let precheck: TripUploadPrecheck
    private let timing: Timing
    @ObservationIgnored private let activity: TransferActivity?
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    @ObservationIgnored private var currentHandle: TransferHandle?
    @ObservationIgnored private var progressWatcher: Task<Void, Never>?
    @ObservationIgnored private var linkWatcher: Task<Void, Never>?
    @ObservationIgnored private var driver: Task<Void, Never>?
    @ObservationIgnored private var started = false
    @ObservationIgnored private var linkUp = true

    public init(
        transport: any DeviceTransport,
        tripName: String,
        deviceName: String,
        precheck: TripUploadPrecheck,
        steps: [QueueStep],
        timing: Timing = Timing(),
        activity: TransferActivity? = nil
    ) {
        self.transport = transport
        self.tripName = tripName
        self.deviceName = deviceName
        self.precheck = precheck
        self.steps = steps
        self.stepCount = steps.count
        self.timing = timing
        self.activity = activity
    }

    // MARK: Derived lines

    public var fraction: Double { progress.fraction }

    public var percentLine: String { "\(Int((progress.fraction * 100).rounded()))%" }

    public var sizeLine: String {
        OBCFormat.transferSizeLine(bytesDone: progress.bytesDone, totalBytes: progress.total, hasWaypoints: false)
    }

    /// The current step's title (a stage name, or "Trip details" for the trip
    /// object) — `nil` in a terminal phase.
    public var currentStepTitle: String? {
        guard stepIndex < steps.count else { return nil }
        return steps[stepIndex].title
    }

    /// "Stage 2 of 5 — Devil's Lake" — the queued-mode header over the per
    /// transfer bar. Counts every step (skips + trip object) in the denominator.
    public var stageProgressLabel: String {
        let position = min(stepIndex + 1, stepCount)
        let title = currentStepTitle ?? tripName
        return "Stage \(position) of \(stepCount) — \(title)"
    }

    /// The done-state tally — "3 uploaded · 1 already on device".
    public var doneTally: String {
        var parts = ["\(committedCount) uploaded"]
        if skippedCount > 0 { parts.append("\(skippedCount) already on device") }
        return parts.joined(separator: " · ")
    }

    // MARK: Failure copy

    public var failedTitle: String {
        switch failure {
        case .storagePrecheck, .device(.storageFull): "Device storage full"
        default: "Couldn't upload trip"
        }
    }

    public var failedMessage: String {
        switch failure {
        case .storagePrecheck(let deficit):
            let noun = deficit == 1 ? "route" : "routes"
            return "\(tripName) needs \(deficit) more \(noun) than \(deviceName) has room for. Delete routes on the device to make room, then upload the trip again."
        case .device(.storageFull):
            return "\(deviceName)'s storage filled up mid-upload. Delete routes on the device to make room, then upload the trip again."
        default:
            return "\(deviceName) didn't answer. Check that it's awake and nearby, then upload the trip again."
        }
    }

    // MARK: Lifecycle

    public func start() {
        guard !started else { return }
        started = true

        // Precheck before any bytes (issue #657): a trip that can't fit fails
        // upfront — never a partial upload that hits storageFull at the last stage.
        guard precheck.fits else {
            phase = .failed
            failure = .storagePrecheck(routeDeficit: precheck.routeSlotDeficit)
            return
        }

        setActive(true)
        // The link watcher spans the whole queue: a drop mid-stage stalls the
        // current transfer (restart-current-stage), a regain lets the tick watcher
        // flip back to uploading.
        linkWatcher = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                let dropped = state == .outOfRange || state == .disconnected
                linkUp = !dropped
                if dropped, phase == .uploading, currentHandle?.currentOutcome == nil {
                    phase = .interrupted
                    setActive(false)
                }
            }
        }
        driver = Task { [weak self] in await self?.runQueue() }
    }

    /// Restart the current stage's transfer after a drop (uploads restart, not
    /// resume) — the trip queue's Resume.
    public func resume() {
        guard phase == .interrupted else { return }
        currentHandle?.resume()
        phase = .uploading
        setActive(true)
    }

    /// Cancel the whole trip upload — aborts the in-flight transfer; completed
    /// stages stay committed on the device (re-running is idempotent).
    public func cancel() {
        currentHandle?.cancel()
    }

    /// Done / a failure's Close.
    public func dismiss() { shouldDismiss = true }

    /// The sheet left the screen — cancel an unresolved transfer and stop the
    /// watchers (a completed queue's dismissal passes through untouched).
    public func sheetDismissed() {
        if let currentHandle, currentHandle.currentOutcome == nil { currentHandle.cancel() }
        tearDown()
        setActive(false)
    }

    deinit {
        driver?.cancel()
        linkWatcher?.cancel()
        progressWatcher?.cancel()
    }

    // MARK: Queue driver

    private func runQueue() async {
        while stepIndex < steps.count {
            if Task.isCancelled { return }
            let step = steps[stepIndex]
            if step.skip {
                skippedCount += 1
                stepIndex += 1
                continue
            }
            guard let (handle, committedCRC) = step.makeTransfer?() else {
                // Nothing resolvable to send (e.g. a trip with no on-device
                // stages) — treat as a skip and move on.
                stepIndex += 1
                continue
            }
            currentHandle = handle
            phase = .uploading
            watchProgress(handle)
            // `handle.outcome` stays unresolved across a drop (the transfer is
            // restartable) — this awaits through interrupt → resume until the
            // stage truly finishes, fails, or is canceled.
            let outcome = await handle.outcome
            progressWatcher?.cancel()
            progressWatcher = nil
            switch outcome {
            case .completed:
                let objectID = await handle.assignedObjectID
                step.commit?(objectID, committedCRC)
                committedCount += 1
                stepIndex += 1
            case .canceled:
                setActive(false)
                shouldDismiss = true
                return
            case .failed(let error):
                failure = .device(error)
                phase = .failed
                setActive(false)
                return
            }
        }
        // Whole queue landed.
        phase = .done
        setActive(false)
        try? await Task.sleep(for: timing.doneAutoDismiss)
        shouldDismiss = true
    }

    private func watchProgress(_ handle: TransferHandle) {
        progressWatcher?.cancel()
        progressWatcher = Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                // A tick while interrupted (and the link is back) means the
                // restart is moving — flip to uploading and re-claim the ledger.
                if phase == .interrupted, linkUp {
                    phase = .uploading
                    setActive(true)
                }
            }
        }
    }

    private func tearDown() {
        driver?.cancel(); driver = nil
        linkWatcher?.cancel(); linkWatcher = nil
        progressWatcher?.cancel(); progressWatcher = nil
    }

    private func setActive(_ active: Bool) {
        if active {
            guard activityToken == nil else { return }
            activityToken = activity?.begin()
        } else if let token = activityToken {
            activityToken = nil
            activity?.end(token)
        }
    }
}
