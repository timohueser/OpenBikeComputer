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
        /// The pre-transfer confirm (epic #638) — the trip + the **Auto-delete**
        /// row, seeded from the app default and changeable before any bytes, then
        /// **Upload trip**. The queue (precheck included) starts on that tap, not on
        /// present, so the chosen level is set *before* the first stage commits. A
        /// trip is one unit: its level applies to **every** member route, overriding
        /// any per-route choice. Skipped straight to `.uploading` for a device with
        /// no retention capability, which has nothing to configure.
        case ready
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
        /// Commit the landed object's link. The third argument is the **trip's**
        /// chosen retention (epic #638), read from the model at execution time and
        /// passed to every stage's commit so the trip level overrides each member
        /// route's own choice (a trip is one unit). The trip-object step ignores it
        /// (trips carry no retention).
        let commit: (@MainActor @Sendable (DeviceObjectID?, UInt32, Retention) -> Void)?
        /// Apply the trip's chosen retention to a **skipped** member stage (finding
        /// #876-4): a skip skips the *bytes*, never the retention postcondition. The
        /// trip-object skip carries none (trips have no retention). Read at execution
        /// time so it uses the confirmed trip level.
        let applyRetention: (@MainActor @Sendable (Retention) -> Void)?

        /// A stage already up-to-date on the device — no bytes transfer, but a member
        /// stage still applies the trip's retention (`applyRetention`); the trip
        /// object's own skip passes `nil`.
        public static func skip(
            title: String, applyRetention: (@MainActor @Sendable (Retention) -> Void)? = nil
        ) -> QueueStep {
            QueueStep(title: title, skip: true, makeTransfer: nil, commit: nil, applyRetention: applyRetention)
        }

        /// A transfer step (a stage upload or the trip object).
        public static func transfer(
            title: String,
            makeTransfer: @escaping @MainActor @Sendable () -> (handle: TransferHandle, committedCRC: UInt32)?,
            commit: @escaping @MainActor @Sendable (DeviceObjectID?, UInt32, Retention) -> Void
        ) -> QueueStep {
            QueueStep(title: title, skip: false, makeTransfer: makeTransfer, commit: commit, applyRetention: nil)
        }
    }

    // MARK: Observable state

    public private(set) var phase: Phase
    public private(set) var progress = TransferProgress(bytesDone: 0, total: 1)
    public private(set) var failure: Failure?
    public private(set) var shouldDismiss = false
    /// The retention every member route lands under (epic #638) — seeded from the
    /// app default, changeable in the `.ready` confirm, applied to the whole trip.
    /// Meaningless when `supportsRetention` is false (the row is hidden).
    public private(set) var retention: Retention
    /// The live link state — the `.ready` confirm's Upload button dims when the
    /// link drops (the route sheet's rule), matching the trip page's Upload button.
    public private(set) var connection: ConnectionState = .connected
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
    /// Whether the connected device honours retention (epic #638) — hides the
    /// Auto-delete row and skips the `.ready` confirm when false (nothing to set).
    public let supportsRetention: Bool

    /// Whether the `.ready` confirm's Upload button can act right now (link up).
    public var canUpload: Bool { connection == .connected }

    // MARK: Wiring

    private let transport: any DeviceLink
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
        transport: any DeviceLink,
        tripName: String,
        deviceName: String,
        precheck: TripUploadPrecheck,
        steps: [QueueStep],
        retention: Retention = .appDefault,
        supportsRetention: Bool = true,
        timing: Timing = Timing(),
        activity: TransferActivity? = nil
    ) {
        self.transport = transport
        self.tripName = tripName
        self.deviceName = deviceName
        self.precheck = precheck
        self.steps = steps
        self.stepCount = steps.count
        self.retention = retention
        self.supportsRetention = supportsRetention
        self.timing = timing
        self.activity = activity
        // A retention-capable device opens on the `.ready` confirm so the level is
        // chosen before any bytes; without it there's nothing to configure, so the
        // queue starts straight away (the prior behaviour).
        self.phase = supportsRetention ? .ready : .uploading
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

        // The link watcher spans the whole sheet: it tracks `connection` so the
        // `.ready` confirm's Upload button dims on a drop, and once the queue is
        // moving a drop mid-stage stalls the current transfer (restart-current-stage),
        // a regain lets the tick watcher flip back to uploading.
        linkWatcher = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
                let dropped = state == .outOfRange || state == .disconnected
                linkUp = !dropped
                if dropped, phase == .uploading, currentHandle?.currentOutcome == nil {
                    phase = .interrupted
                    setActive(false)
                }
            }
        }

        // A capable device holds on `.ready` (the Auto-delete confirm) until the
        // rider taps Upload; without capability there's nothing to configure, so
        // the queue begins now (the prior behaviour, row hidden).
        if phase != .ready { beginQueue() }
    }

    /// Pick the trip's Auto-delete level in the `.ready` confirm (epic #638) — a
    /// plain setter; the value rides to every member route's commit.
    public func selectRetention(_ retention: Retention) {
        self.retention = retention
    }

    /// Leave the `.ready` confirm and start the queued upload (the Upload button).
    /// No-op once the queue is already under way.
    public func beginUpload() {
        guard phase == .ready else { return }
        beginQueue()
    }

    /// Precheck, then start the queue. The precheck runs before any bytes (issue
    /// #657): a trip that can't fit fails upfront — never a partial upload that
    /// hits storageFull at the last stage.
    private func beginQueue() {
        guard precheck.fits else {
            phase = .failed
            failure = .storagePrecheck(routeDeficit: precheck.routeSlotDeficit)
            return
        }
        phase = .uploading
        setActive(true)
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
                // Skipped bytes, not skipped policy (finding #876-4): a member stage
                // already current on the device still lands the trip's chosen
                // retention — the same postcondition a freshly uploaded stage gets.
                step.applyRetention?(retention)
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
                // The trip's chosen retention rides to every stage commit, read
                // here at execution time (after the rider confirmed) — it overrides
                // each member route's own level (a trip is one unit).
                step.commit?(objectID, committedCRC, retention)
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
