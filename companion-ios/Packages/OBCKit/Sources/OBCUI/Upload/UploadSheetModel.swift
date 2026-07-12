import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the upload sheet (B5) — the bottom sheet that owns a route push
/// from tap to terminal state, over whichever detail dressing launched it:
///
///   • `.uploading`   — F, the bar tracking `TransferHandle.progress`
///   • `.interrupted` — the link dropped mid-transfer; **Resume** restarts the
///                      upload from scratch (uploads restart, not resume — the
///                      device discarded its partial)
///   • `.done`        — F₂, the brief confirm (auto-dismisses, or tap Done)
///   • `.failed`      — the transfer failed for good (H4 no link, or the
///                      device rejected the object)
///
/// The drop signal is `DeviceTransport.state` → `.outOfRange` **or**
/// `.disconnected` (both are a drop, matching `MainScreenModel`'s sync watch) —
/// the handle's progress stream stays open, stalled (see `TransferHandle`).
/// Cancel aborts the transfer; the sheet leaves no upload running behind it
/// (`sheetDismissed()` cancels an unresolved handle).
@MainActor @Observable
public final class UploadSheetModel {
    public enum Phase: Equatable {
        case uploading
        case interrupted
        case done
        case failed
    }

    /// Pacing knobs, injectable so tests don't wait design-time holds.
    public struct Timing: Sendable {
        /// How long F₂ holds before the sheet dismisses itself.
        public var doneAutoDismiss: Duration

        public init(doneAutoDismiss: Duration = .seconds(2.6)) {
            self.doneAutoDismiss = doneAutoDismiss
        }
    }

    // MARK: Observable state

    public private(set) var phase: Phase = .uploading
    public private(set) var progress: TransferProgress
    /// Why the transfer failed — set alongside `.failed` so the failure copy can
    /// speak to the actual cause (storage-full vs. a generic no-answer failure).
    /// `nil` in every non-failed phase.
    public private(set) var failure: DeviceError?
    /// Flips when the sheet should go away — a finished cancel, F₂'s Done /
    /// auto-dismiss, or Close on a failure. The view observes and dismisses.
    public private(set) var shouldDismiss = false

    // MARK: Fixed facts

    public let routeName: String
    public let deviceName: String
    /// Whether the size readout says "route + waypoints" or just "route".
    private let hasWaypoints: Bool

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let blob: RouteBlob
    private let timing: Timing
    /// Fires once when the upload completes, carrying the **device-assigned object
    /// id** (nil if the device didn't report one) and the committed payload's
    /// CRC-32 (the `OnDeviceState` fingerprint) — the E1 landing saves the route
    /// here ("Uploading saves it too") and records it as on-device, up to date.
    private let onCompleted: (DeviceObjectID?, UInt32) -> Void
    /// The foreground-only policy's in-flight ledger (#459) — `nil` in tests
    /// and previews that don't exercise the lifecycle.
    @ObservationIgnored private let activity: TransferActivity?
    /// This upload's claim while an attempt is actually moving bytes. Released
    /// on a drop (`.interrupted` — a stalled upload must not hold the
    /// background grace window; it restarts after the foreground reconnect),
    /// on any terminal outcome, and on sheet teardown; re-claimed by `resume()`.
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var watchers: [Task<Void, Never>] = []
    @ObservationIgnored private var started = false
    /// The drop watcher's running view of the link. The tick watcher reads it
    /// to tell a genuine resume tick from a stale pre-drop one: ticks and link
    /// states arrive on two independent streams, so a backlogged tick can be
    /// delivered *after* the drop it preceded.
    @ObservationIgnored private var linkUp = true

    public init(
        transport: any DeviceTransport,
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
    }

    // MARK: Derived lines (design F)

    public var fraction: Double { progress.fraction }

    /// "64%" — mono forest, right of the title block.
    public var percentLine: String {
        "\(Int((progress.fraction * 100).rounded()))%"
    }

    /// "1.4 / 2.3 MB · route + waypoints" — plain-English MB, never byte counts.
    public var sizeLine: String {
        OBCFormat.transferSizeLine(
            bytesDone: progress.bytesDone,
            totalBytes: progress.total,
            hasWaypoints: hasWaypoints
        )
    }

    // MARK: Failure copy (design — Couldn't upload)

    /// The failure card's heading — cause-specific so a storage-full reject reads
    /// as a device-storage problem, not a lost link.
    public var failedTitle: String {
        failure == .storageFull ? "Device storage full" : "Couldn't upload"
    }

    /// The failure card's body. Storage-full gets actionable copy (delete routes
    /// on the device); everything else keeps the "device didn't answer" framing.
    /// The storage-full line deliberately says nothing about *updating* an
    /// existing route — the device exempts replace-by-id uploads from the cap.
    public var failedMessage: String {
        failure == .storageFull
            ? "\(deviceName)'s route storage is full. Delete routes on the device to make room, then try again."
            : "\(deviceName) didn't answer. Check that it's awake and nearby, then try again."
    }

    // MARK: Lifecycle

    /// Start the transfer and watch it (call once, from `.task`).
    public func start() {
        guard !started else { return }
        started = true
        setTransferActive(true)

        let handle = transport.uploadRoute(blob)
        self.handle = handle

        // Progress ticks. A tick is also the proof a resume is moving again —
        // but only while the link is up: a stale pre-drop tick delivered after
        // the drop event must not flip the sheet back to `.uploading` (and
        // re-claim the ledger) for a transfer whose link is already gone.
        watchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                if phase == .interrupted, linkUp {
                    phase = .uploading
                    setTransferActive(true)  // moving again — re-claim
                }
            }
        })

        // The drop signal: the link leaves the transfer stalled-but-resumable.
        // Both `.outOfRange` and `.disconnected` count as a drop (the sync
        // watch in `MainScreenModel` treats them the same) — otherwise a link
        // that drops straight to `.disconnected` wedges the sheet in `.uploading`.
        watchers.append(Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                let dropped = state == .outOfRange || state == .disconnected
                linkUp = !dropped
                if dropped, phase == .uploading, handle.currentOutcome == nil {
                    phase = .interrupted
                    // Stalled, not moving — release the ledger claim so the
                    // background grace window doesn't wait on a transfer whose
                    // link is already gone.
                    setTransferActive(false)
                }
            }
        })

        // Terminal state — never inferred from byte counts.
        watchers.append(Task { [weak self] in
            let outcome = await handle.outcome
            // `sheetDismissed()` cancels the watchers but deliberately leaves a
            // resolved handle alone — so a completion that raced the dismiss
            // resumes this `await` immediately. Bail before acting on it, or the
            // `.completed` branch re-saves the route and re-arms `shouldDismiss`
            // on an already-torn-down sheet.
            guard let self, !Task.isCancelled else { return }
            setTransferActive(false)  // terminal either way — release the claim
            switch outcome {
            case .completed:
                phase = .done
                onCompleted(await handle.assignedObjectID, CRC32.checksum(blob.payload))
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

    /// Cancel upload — aborts the transfer on both ends; the resolved outcome
    /// flips `shouldDismiss`. Works mid-transfer and from `.interrupted`.
    public func cancel() {
        handle?.cancel()
    }

    /// Restart a dropped transfer from scratch (F interrupted).
    public func resume() {
        guard phase == .interrupted else { return }
        handle?.resume()
        // Optimistic — the next tick confirms; a second drop re-interrupts.
        phase = .uploading
        setTransferActive(true)
    }

    /// F₂'s Done / a failure's Close.
    public func dismiss() {
        shouldDismiss = true
    }

    /// The sheet left the screen. A still-unresolved transfer must not keep
    /// running headless behind the detail — cancel it (no-op after a terminal
    /// outcome, so the normal post-done dismissal passes through).
    public func sheetDismissed() {
        if let handle, handle.currentOutcome == nil { handle.cancel() }
        watchers.forEach { $0.cancel() }
        watchers.removeAll()
        // The cancel above resolves the outcome, but its watcher just died —
        // release the ledger claim here so a torn-down sheet can't hold the
        // background grace window open.
        setTransferActive(false)
    }

    /// Backstop for a model released without `sheetDismissed()` — the watchers
    /// are `[weak self]` so they never pin the model, but leaving them running
    /// would keep them consuming stream events for the session.
    deinit {
        watchers.forEach { $0.cancel() }
    }

    /// The #459 ledger claim, idempotent in both directions — the release runs
    /// from several exits (drop-watch, terminal outcome, sheet teardown) that
    /// can race pairwise.
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
