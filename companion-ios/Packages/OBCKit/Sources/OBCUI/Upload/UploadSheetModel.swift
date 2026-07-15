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
        /// The pre-transfer confirm (epic #638 S7): the route + size + the
        /// **Auto-delete** row, seeded from the app default and changeable before
        /// the push, then **Upload**. The transfer starts on that tap, not on
        /// present — so the chosen retention is set *before* the upload (S6's
        /// post-commit `setRouteRetention` sends it). Skipped straight to
        /// `.uploading` for a device with no retention capability, which has
        /// nothing to configure here.
        case ready
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

    public private(set) var phase: Phase
    public private(set) var progress: TransferProgress
    /// The desired retention for this route (epic #638 S7) — seeded from the app
    /// default (or the route's existing choice), changeable in the `.ready` phase,
    /// carried to `onCompleted` so the commit push sends it. Meaningless when
    /// `supportsRetention` is false (the row is hidden).
    public private(set) var retention: Retention
    /// The live link state — the `.ready` phase's Upload button dims when the
    /// link drops (the S4 rule), matching the detail's Upload button.
    public private(set) var connection: ConnectionState = .connected
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
    /// Whether the connected device honours retention (epic #638) — hides the
    /// Auto-delete row and skips the `.ready` confirm when false (nothing to set).
    public let supportsRetention: Bool
    /// Whether the size readout says "route + waypoints" or just "route".
    private let hasWaypoints: Bool

    /// Whether the `.ready` phase's Upload button can act right now (link up).
    public var canUpload: Bool { connection == .connected }

    /// The total payload size line for the `.ready` confirm ("24 kB · route +
    /// waypoints") — the transfer readout without the running "done /" prefix.
    public var readySizeLine: String {
        OBCFormat.transferTotalSizeLine(totalBytes: progress.total, hasWaypoints: hasWaypoints)
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let blob: RouteBlob
    private let timing: Timing
    /// Fires once when the upload completes, carrying the **device-assigned object
    /// id** (nil if the device didn't report one), the committed payload's CRC-32
    /// (the `OnDeviceState` fingerprint), and the rider's chosen **retention**
    /// (epic #638 — S6's post-commit `setRouteRetention` sends it) — the E1 landing
    /// saves the route here ("Uploading saves it too") and records it as on-device,
    /// up to date. Contract: fires **before** `phase` reads `.done` — an observer
    /// that sees F₂ can rely on the save having happened.
    private let onCompleted: (DeviceObjectID?, UInt32, Retention) -> Void
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
        retention: Retention = .appDefault,
        supportsRetention: Bool = true,
        timing: Timing = Timing(),
        activity: TransferActivity? = nil,
        onCompleted: @escaping (DeviceObjectID?, UInt32, Retention) -> Void = { _, _, _ in }
    ) {
        self.transport = transport
        self.blob = blob
        self.routeName = blob.summary.name
        self.deviceName = deviceName
        self.retention = retention
        self.supportsRetention = supportsRetention
        self.hasWaypoints = !blob.waypoints.isEmpty
        self.timing = timing
        self.activity = activity
        self.onCompleted = onCompleted
        self.progress = TransferProgress(bytesDone: 0, total: blob.payload.count)
        // A device with retention capability opens on the `.ready` confirm so the
        // Auto-delete level is chosen before the push; without it there's nothing
        // to configure, so the transfer starts straight away (the prior behaviour).
        self.phase = supportsRetention ? .ready : .uploading
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

    /// Set up the sheet (call once, from `.task`): watch the link so the `.ready`
    /// confirm's Upload button dims on a drop, then either hold on `.ready`
    /// (retention-capable) or begin the transfer immediately (no capability — the
    /// prior behaviour). The transfer machinery lives in ``beginUpload()``.
    public func start() {
        guard !started else { return }
        started = true
        watchers.append(Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
            }
        })
        if phase != .ready { beginUpload() }
    }

    /// Pick the Auto-delete level in the `.ready` confirm (epic #638 S7) — a plain
    /// setter; the value rides to the commit push via `onCompleted`.
    public func selectRetention(_ retention: Retention) {
        self.retention = retention
    }

    /// Leave the `.ready` confirm and start the transfer (the Upload button).
    /// No-op once the transfer is already under way.
    public func beginUpload() {
        guard handle == nil else { return }
        if phase == .ready { phase = .uploading }
        setTransferActive(true)

        let handle = transport.uploadRoute(blob)
        self.handle = handle

        // Progress ticks. A tick is also the proof a resume is moving again —
        // but only while the link is up. Ticks ride their own stream, so a
        // backlogged one can land after the phase already moved on; a stale
        // tick must not flip the sheet back to `.uploading` (and re-claim the
        // ledger), move the parked bar, or disturb the settled one (`.done`
        // snapped it to 100%).
        watchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                if phase == .done || phase == .failed { continue }
                if phase == .interrupted, !linkUp { continue }
                progress = tick
                if phase == .interrupted {
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
                // The assigned id resolves with the outcome on BLE but a task-hop
                // after it on the mock — await it *before* F₂, so `.done` is only
                // observable once `onCompleted` (the E1 save) has already run.
                let assignedID = await handle.assignedObjectID
                guard !Task.isCancelled else { return }
                // The final tick rides a separate stream and can land after the
                // outcome — snap the bar so `.done` always reads 100%.
                progress = TransferProgress(bytesDone: progress.total, total: progress.total)
                onCompleted(assignedID, CRC32.checksum(blob.payload), retention)
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
