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
    private let onCompleted: (UInt16?, UInt32) -> Void
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var watchers: [Task<Void, Never>] = []
    @ObservationIgnored private var started = false

    public init(
        transport: any DeviceTransport,
        blob: RouteBlob,
        deviceName: String,
        timing: Timing = Timing(),
        onCompleted: @escaping (UInt16?, UInt32) -> Void = { _, _ in }
    ) {
        self.transport = transport
        self.blob = blob
        self.routeName = blob.summary.name
        self.deviceName = deviceName
        self.hasWaypoints = !blob.waypoints.isEmpty
        self.timing = timing
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

    // MARK: Lifecycle

    /// Start the transfer and watch it (call once, from `.task`).
    public func start() {
        guard !started else { return }
        started = true

        let handle = transport.uploadRoute(blob)
        self.handle = handle

        // Progress ticks. A tick is also the proof a resume is moving again.
        watchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                if phase == .interrupted { phase = .uploading }
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
                if dropped, phase == .uploading, handle.currentOutcome == nil {
                    phase = .interrupted
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
            switch outcome {
            case .completed:
                phase = .done
                onCompleted(await handle.assignedObjectID, CRC32.checksum(blob.payload))
                try? await Task.sleep(for: timing.doneAutoDismiss)
                shouldDismiss = true
            case .canceled:
                shouldDismiss = true
            case .failed:
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
    }
}
