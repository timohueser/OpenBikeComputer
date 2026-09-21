import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the upload sheet: the bottom sheet that owns a route push from tap to terminal
/// state, over whichever detail dressing launched it.
///
/// A drop is `.outOfRange` or `.disconnected`, and the handle's progress stream then stays open
/// but stalled. Resume restarts the upload from scratch, because the device discarded its partial.
/// Cancel aborts the transfer, and the sheet leaves no upload running behind it.
@MainActor @Observable
public final class UploadSheetModel {
    public enum Phase: Equatable {
        case uploading
        case interrupted
        case done
        case failed
    }

    /// Pacing knobs, injectable so tests do not wait out design-time holds.
    public struct Timing: Sendable {
    /// How long the confirm holds before the sheet dismisses itself.
        public var doneAutoDismiss: Duration

        public init(doneAutoDismiss: Duration = .seconds(2.6)) {
            self.doneAutoDismiss = doneAutoDismiss
        }
    }

    // MARK: Observable state

    public private(set) var phase: Phase
    public private(set) var progress: TransferProgress
    /// Why the transfer failed, set alongside `.failed` so the copy can speak to the actual
    /// cause. Nil in every other phase.
    public private(set) var failure: DeviceError?
    /// Flips when the sheet should go away. The view observes it and dismisses.
    public private(set) var shouldDismiss = false

    // MARK: Fixed facts

    public let routeName: String
    public let deviceName: String
    /// Whether the size readout says "route + waypoints" or just "route".
    private let hasWaypoints: Bool
    // MARK: Wiring

    private let transport: any DeviceLink & DeviceObjects
    private let blob: RouteBlob
    private let timing: Timing
    private let onCompleted: (DeviceObjectID?, UInt32) -> Void
    /// The foreground-only policy's in-flight ledger. Nil in tests and previews.
    @ObservationIgnored private let activity: TransferActivity?
    /// This upload's claim while an attempt is actually moving bytes. Released on a drop, because
    /// a stalled upload must not hold the background grace window open, on any terminal outcome,
    /// and on sheet teardown. `resume()` re-claims it.
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var watchers: [Task<Void, Never>] = []
    @ObservationIgnored private var started = false
    /// The drop watcher's running view of the link. The tick watcher reads it to tell a genuine
    /// resume tick from a stale pre-drop one: ticks and link states arrive on two independent
    /// streams, so a backlogged tick can be delivered after the drop it preceded.
    @ObservationIgnored private var linkUp = true

    public init(
        transport: any DeviceLink & DeviceObjects,
        blob: RouteBlob,
        deviceName: String,
        timing: Timing = Timing(),
        activity: TransferActivity? = nil,
        onCompleted: @escaping (DeviceObjectID?, UInt32) -> Void = { _, _ in }
    ) {
        self.transport = transport
        self.blob = blob
        self.routeName = blob.summary.name
        self.deviceName = deviceName
        self.hasWaypoints = !blob.waypoints.isEmpty
        self.timing = timing
        self.activity = activity
        self.onCompleted = onCompleted
        self.progress = TransferProgress(bytesDone: 0, total: blob.payload.count)
        self.phase = .uploading
    }

    // MARK: Derived lines

    public var fraction: Double { progress.fraction }

    public var percentLine: String {
        "\(Int((progress.fraction * 100).rounded()))%"
    }

    /// Plain-English megabytes, never byte counts.
    public var sizeLine: String {
        OBCFormat.transferSizeLine(
            bytesDone: progress.bytesDone,
            totalBytes: progress.total,
            hasWaypoints: hasWaypoints
        )
    }

    // MARK: Failure copy

    /// The failure card's heading, cause-specific so a storage-full reject reads as a device
    /// storage problem and not a lost link.
    public var failedTitle: String {
        failure == .storageFull ? "Device storage full" : "Couldn't upload"
    }

    /// The failure card's body. Storage-full gets actionable copy; everything else keeps the
    /// "device didn't answer" framing. The storage-full line deliberately says nothing about
    /// updating an existing route, because the device exempts replace-by-id uploads from the cap.
    public var failedMessage: String {
        failure == .storageFull
            ? "\(deviceName)'s route storage is full. Delete routes on the device to make room, then try again."
            : "\(deviceName) didn't answer. Check that it's awake and nearby, then try again."
    }

    // MARK: Lifecycle

    public func start() {
        guard !started else { return }
        started = true
        beginUpload()
    }
    private func beginUpload() {
        guard handle == nil else { return }

        setTransferActive(true)

        let handle = transport.uploadRoute(blob)
        self.handle = handle

        // Progress ticks. A tick is also the proof a resume is moving again, but only while the
        // link is up. Ticks ride their own stream, so a backlogged one can land after the phase
        // moved on, and a stale tick must not flip the sheet back to `.uploading`, move the parked
        // bar, or disturb the settled one.
        watchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                if phase == .done || phase == .failed { continue }
                if phase == .interrupted, !linkUp { continue }
                progress = tick
                if phase == .interrupted {
                    phase = .uploading
                    setTransferActive(true)  // moving again, so re-claim
                }
            }
        })

        // The drop signal: the link leaves the transfer stalled but resumable. Both `.outOfRange`
        // and `.disconnected` count, or a link that drops straight to `.disconnected` would wedge
        // the sheet in `.uploading`.
        watchers.append(Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                let dropped = state == .outOfRange || state == .disconnected
                linkUp = !dropped
                if dropped, phase == .uploading, handle.currentOutcome == nil {
                    phase = .interrupted
                    // Stalled, not moving: release the ledger claim, so the background grace
                    // window does not wait on a transfer whose link is already gone.
                    setTransferActive(false)
                }
            }
        })

        // Terminal state, never inferred from byte counts.
        watchers.append(Task { [weak self] in
            let outcome = await handle.outcome
            // `sheetDismissed()` cancels the watchers but deliberately leaves a resolved handle
            // alone, so a completion that raced the dismiss resumes this await immediately. Bail
            // before acting on it, or the completed branch re-saves the route on a torn-down sheet.
            guard let self, !Task.isCancelled else { return }
            setTransferActive(false)  // terminal either way, so release the claim
            switch outcome {
            case .completed:
                // The assigned id resolves with the outcome on BLE but a task-hop after it on the
                // mock. Await it before the confirm, so `.done` is only observable once
                // `onCompleted` has already run.
                let assignedID = await handle.assignedObjectID
                guard !Task.isCancelled else { return }
                // The final tick rides a separate stream and can land after the outcome, so snap
                // the bar and `.done` always reads 100%.
                progress = TransferProgress(bytesDone: progress.total, total: progress.total)
                onCompleted(assignedID, CRC32.checksum(blob.payload))
                phase = .done
                try? await Task.sleep(for: timing.doneAutoDismiss)
                shouldDismiss = true
            case .canceled:
                shouldDismiss = true
            case .failed(let error):
                failure = error
                phase = .failed
            }
        })
    }

    /// Abort the transfer on both ends; the resolved outcome flips `shouldDismiss`. Works
    /// mid-transfer and from `.interrupted`.
    public func cancel() {
        handle?.cancel()
    }

    /// Restart a dropped transfer from scratch.
    public func resume() {
        guard phase == .interrupted else { return }
        handle?.resume()
        // Optimistic: the next tick confirms, and a second drop re-interrupts.
        phase = .uploading
        setTransferActive(true)
    }

    public func dismiss() {
        shouldDismiss = true
    }

    /// The sheet left the screen. A still-unresolved transfer must not keep running headless
    /// behind the detail, so cancel it. A no-op after a terminal outcome.
    public func sheetDismissed() {
        if let handle, handle.currentOutcome == nil { handle.cancel() }
        watchers.forEach { $0.cancel() }
        watchers.removeAll()
        // The cancel above resolves the outcome, but its watcher just died, so release the ledger
        // claim here and a torn-down sheet cannot hold the background grace window open.
        setTransferActive(false)
    }

    /// Backstop for a model released without `sheetDismissed()`. The watchers capture self weakly,
    /// so they never pin the model, but leaving them running would consume stream events all
    /// session.
    deinit {
        watchers.forEach { $0.cancel() }
    }

    /// The ledger claim, idempotent in both directions, because the release runs from several
    /// exits that can race pairwise.
    private func setTransferActive(_ active: Bool) {
        if active {
            guard activityToken == nil else { return }
            activityToken = activity?.begin()
        } else if let token = activityToken {
            activityToken = nil
            activity?.end(token)
        }
    }
}
