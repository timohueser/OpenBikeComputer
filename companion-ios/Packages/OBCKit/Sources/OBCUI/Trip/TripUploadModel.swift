import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Drives a whole-trip upload: the queued sibling of `UploadSheetModel`. One transfer in flight
/// at a time, in ride order: each day route is skipped when it is already up to date, replaced in
/// place when it is on the device but outdated, or freshly uploaded when it is absent. Then the
/// trip object, last. The precheck runs before any bytes, so a trip that cannot fit fails up front
/// with guidance rather than hitting a full device at day four.
///
/// Interruption keeps `UploadSheetModel`'s restart-the-current-step semantics, completed days
/// stay committed, and re-running is idempotent, because the skips catch everything already
/// landed. Each object's link is committed the instant its transfer lands, exactly like a single
/// upload.
@MainActor @Observable
public final class TripUploadModel: Identifiable {
    public nonisolated let id = UUID()

    public enum Phase: Equatable, Sendable {
        case uploading
        case interrupted
        case done
        case failed
    }

    /// Why the whole-trip upload failed: a precheck deficit before any bytes, or a device reject
    /// mid-queue. Drives the failure copy.
    public enum Failure: Equatable, Sendable {
        /// The precheck found the trip cannot fit, by this many route slots.
        case storagePrecheck(routeDeficit: Int)
        /// A device transfer failed for good.
        case device(DeviceError)
    }

    public struct Timing: Sendable {
        public var doneAutoDismiss: Duration
        public init(doneAutoDismiss: Duration = .seconds(2.6)) {
            self.doneAutoDismiss = doneAutoDismiss
        }
    }

    /// One queue step: a skipped day with no bytes, or a transfer of a day route or the trip object.
    /// `makeTransfer` is evaluated at execution time, so the trip-object step reads the day route ids
    /// the just-committed days landed under. It returns nil to degenerate to a skip.
    public struct QueueStep: Sendable {
        let title: String
        let skip: Bool
        let makeTransfer: (@MainActor @Sendable () -> (handle: TransferHandle, committedCRC: UInt32)?)?
        let commit: (@MainActor @Sendable (DeviceObjectID?, UInt32) -> Void)?
        var run: (@MainActor @Sendable () async throws -> Void)?

        public static func skip(
            title: String
        ) -> QueueStep {
            QueueStep(title: title, skip: true, makeTransfer: nil, commit: nil)
        }

        /// A device command with no bytes to send, such as a delete. A throw fails the queue.
        public static func command(
            title: String, run: @escaping @MainActor @Sendable () async throws -> Void
        ) -> QueueStep {
            QueueStep(title: title, skip: false, makeTransfer: nil, commit: nil, run: run)
        }

        /// A transfer step: a day route upload or the trip object.
        public static func transfer(
            title: String,
            makeTransfer: @escaping @MainActor @Sendable () -> (handle: TransferHandle, committedCRC: UInt32)?,
            commit: @escaping @MainActor @Sendable (DeviceObjectID?, UInt32) -> Void
        ) -> QueueStep {
            QueueStep(title: title, skip: false, makeTransfer: makeTransfer, commit: commit)
        }
    }

    // MARK: Observable state

    public private(set) var phase: Phase
    public private(set) var progress = TransferProgress(bytesDone: 0, total: 1)
    public private(set) var failure: Failure?
    public private(set) var shouldDismiss = false
    /// The live link state for the current transfer.
    public private(set) var connection: ConnectionState = .connected
    /// The current queue step, zero-based, which drives the header.
    public private(set) var stepIndex = 0
    /// Days skipped because the device already held them.
    public private(set) var skippedCount = 0
    /// Objects committed so far, days and trip.
    public private(set) var committedCount = 0

    // MARK: Fixed facts

    /// What the device's TRIP RECEIVED card shows once the trip lands.
    public let card: DeviceTripCard
    public var tripName: String { card.name }
    public let deviceName: String
    /// Total queue steps: the header's denominator.
    public let stepCount: Int
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
        card: DeviceTripCard,
        deviceName: String,
        precheck: TripUploadPrecheck,
        steps: [QueueStep],
        timing: Timing = Timing(),
        activity: TransferActivity? = nil
    ) {
        self.transport = transport
        self.card = card
        self.deviceName = deviceName
        self.precheck = precheck
        self.steps = steps
        self.stepCount = steps.count
        self.timing = timing
        self.activity = activity
        self.phase = .uploading
    }

    // MARK: Derived lines

    public var fraction: Double { progress.fraction }

    public var percentLine: String { "\(Int((progress.fraction * 100).rounded()))%" }

    public var sizeLine: String {
        OBCFormat.transferSizeLine(bytesDone: progress.bytesDone, totalBytes: progress.total, hasWaypoints: false)
    }

    /// The current step's title, a day name or the trip object's. Nil in a terminal phase.
    public var currentStepTitle: String? {
        guard stepIndex < steps.count else { return nil }
        return steps[stepIndex].title
    }

    /// The queued-mode line over the bar: "Day 2 · 3 of 4". It counts every step, skips and the
    /// trip object included.
    public var stepProgressLabel: String {
        let position = min(stepIndex + 1, stepCount)
        return "\(currentStepTitle ?? tripName) · \(position) of \(stepCount)"
    }

    // MARK: Failure copy

    public var failedTitle: String {
        switch failure {
        case .storagePrecheck, .device(.storageFull): "\(deviceName) is full"
        default: "Not sent"
        }
    }

    public var failedMessage: String {
        switch failure {
        case .storagePrecheck(let deficit):
            let noun = deficit == 1 ? "route" : "routes"
            return "\(tripName) needs room for \(deficit) more \(noun). Delete routes on \(deviceName), then send again."
        case .device(.storageFull):
            return "There is no room for more routes. Delete routes on \(deviceName), then send again. The days sent so far stay on it."
        default:
            return "\(deviceName) did not answer. Make sure it is on and near your phone, then send again."
        }
    }

    // MARK: Lifecycle

    public func start() {
        guard !started else { return }
        started = true

        // A link drop stalls the current step. Progress after reconnect resumes it.
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

        beginQueue()
    }
    /// Precheck, then start the queue. The precheck runs before any bytes, so a trip that cannot
    /// fit fails up front rather than as a partial upload that fills the device at the last day.
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

    /// Restart the current step's transfer after a drop: uploads restart, they do not resume.
    public func resume() {
        guard phase == .interrupted else { return }
        currentHandle?.resume()
        phase = .uploading
        setActive(true)
    }

    /// Cancel the whole trip upload, aborting the in-flight transfer. Completed days stay
    /// committed on the device, and re-running is idempotent.
    public func cancel() {
        currentHandle?.cancel()
    }

    /// Done / a failure's Close.
    public func dismiss() { shouldDismiss = true }

    /// The sheet left the screen: cancel an unresolved transfer and stop the watchers. A completed
    /// queue's dismissal passes through untouched.
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
            if let run = step.run {
                do {
                    try await run()
                } catch {
                    failure = .device(error as? DeviceError ?? .writeFailed)
                    phase = .failed
                    setActive(false)
                    return
                }
                stepIndex += 1
                continue
            }
            guard let (handle, committedCRC) = step.makeTransfer?() else {
                // Nothing resolvable to send, such as a trip with no on-device days, so treat it
                // as a skip and move on.
                stepIndex += 1
                continue
            }
            currentHandle = handle
            phase = .uploading
            watchProgress(handle)
            // `handle.outcome` stays unresolved across a drop, because the transfer is
            // restartable, so this awaits through the interrupt and the resume until the step
            // truly finishes, fails, or is cancelled.
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
                // A tick while interrupted, with the link back, means the restart is moving, so
                // flip to uploading and re-claim the ledger.
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
