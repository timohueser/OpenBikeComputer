import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the firmware-update screen (S7) — import an `UPDATE.BIN`, stream it
/// to the device as a `fwImage` (spec §7.6), then request the on-glass install
/// (`installFw`, §4.4). Depends only on `DeviceTransport` (the golden rule).
///
/// The state machine:
///
/// ```
///   idle ──stage(valid file)──▶ staged ──send()──▶ transferring ──┐
///     ▲                            ▲                   │  │        │ commit ok
///     │  stage(bad file)→alert     │ cancel            │  │ drop   ▼
///     └────────────────────────────┘                   │  └▶ interrupted ──resume──┐
///                                                       │           │                │
///                                        installFw ok ──┼───────────┴────────────────┘
///                                                       ▼
///                                                awaitingConfirm ──reconnect on new version──▶ done
///                                                       │
///                              installFw busy/noStaged/…│ or transfer failed
///                                                       ▼
///                                                     failed ──send()──▶ transferring
/// ```
///
/// The transfer reuses the route-upload machinery: whole-object restart (a drop
/// re-sends from scratch, never resumes). After the device commits `/UPDATE.BIN`
/// the model sends `installFw`; on `accepted` it enters `awaitingConfirm` — the
/// rider confirms **on the device**, which reboots to install and drops the link.
/// "Done" is detected by the normal reconnect: when the link returns and DIS
/// 0x2A26 now equals the staged version, the update landed. There is no progress
/// for the flash phase — the device is off-link in the bootloader.
@MainActor @Observable
public final class FirmwareUpdateModel {
    public enum Phase: Equatable {
        case idle
        case staged
        case transferring
        case interrupted
        case awaitingConfirm
        case done
        case failed
    }

    // MARK: Observable state

    public private(set) var phase: Phase = .idle {
        // The #459/#754 in-flight ledger claim, held exactly while a send is
        // moving bytes. Mirroring it on the phase transition (like
        // `RideSyncCoordinator.syncState`) covers every exit uniformly: commit
        // → `.awaitingConfirm`, cancel → `.staged`, a drop → `.interrupted`,
        // and the failure branches all release it. A stalled `.interrupted`
        // transfer deliberately drops the claim — the background drain must not
        // wait on a transfer whose link is already gone (Resume restarts it),
        // and the screen needn't stay awake for a send that isn't advancing.
        didSet {
            guard oldValue != phase else { return }
            if phase == .transferring {
                if activityToken == nil { activityToken = activity?.begin() }
            } else if let token = activityToken {
                activityToken = nil
                activity?.end(token)
            }
        }
    }
    public private(set) var progress = TransferProgress(bytesDone: 0, total: 0)
    /// The device's running firmware version (DIS 0x2A26), `nil` until it lands /
    /// while the link is down. Re-read on every reconnect.
    public private(set) var runningVersion: String?
    /// The imported + validated update, `nil` until a good file is staged.
    public private(set) var staged: StagedFirmware?
    public private(set) var connection: ConnectionState = .connecting
    /// A picked file that isn't a usable update — surfaced as an alert, never a
    /// phase (a corrupt download fails in the picker, not on the device).
    /// Settable so the alert's dismissal clears it.
    public var importError: String?
    /// The failure sentence for a `.failed` phase (a mapped `installFw` reply or a
    /// transfer failure). `nil` in every other phase.
    public private(set) var failureMessage: String?

    // MARK: Fixed facts

    public let deviceName: String

    // MARK: Wiring

    private let transport: any DeviceTransport
    /// The foreground-only policy's in-flight ledger (#459), shared with the
    /// upload sheet + ride-sync coordinator. A firmware send claims a token
    /// while `.transferring` so it, too, is drained (not dropped) across a
    /// background transition — and so the #754 idle-timer guard keeps the
    /// screen awake for it. `nil` in previews/tests that don't wire it.
    @ObservationIgnored private let activity: TransferActivity?
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var transferWatchers: [Task<Void, Never>] = []
    @ObservationIgnored private var started = false
    /// Set once the link drops after `installFw` accepted — the reboot is under
    /// way, so the copy switches from "confirm on the device" to "installing".
    @ObservationIgnored private var sawDropSinceInstall = false

    /// A pre-staged update (the `-OBCFirmwareDemo` hook / previews) — validated +
    /// staged on `start()`, since the Files picker can't be driven from automation.
    @ObservationIgnored private let prestage: Data?
    /// Fire Send once the pre-staged file is validated (the `send` demo token) —
    /// so a demo/screenshot run walks the whole flow on its own.
    @ObservationIgnored private let autoSend: Bool

    public init(
        transport: any DeviceTransport,
        deviceName: String,
        activity: TransferActivity? = nil,
        prestage: Data? = nil,
        autoSend: Bool = false
    ) {
        self.transport = transport
        self.deviceName = deviceName
        self.activity = activity
        self.prestage = prestage
        self.autoSend = autoSend
    }

    // MARK: Derived copy

    /// "v0.4.2" — the running-version readout; "—" while unknown.
    public var runningVersionLine: String { Self.versioned(runningVersion) ?? "—" }

    /// "v1.2.0+abc1234" — the staged file's version; empty when nothing staged.
    public var stagedVersionLine: String { Self.versioned(staged?.version) ?? "" }

    /// "854 KB" — the staged file's on-disk size.
    public var stagedSizeLine: String {
        guard let staged else { return "" }
        return ByteCountFormatter.string(fromByteCount: Int64(staged.byteCount), countStyle: .file)
    }

    /// The staged version already matches what's running — offering to send it
    /// again is pointless. `false` when either version is unknown.
    public var stagedMatchesRunning: Bool {
        guard let staged, let runningVersion else { return false }
        return staged.version == runningVersion
    }

    /// Sending needs a validated file and a live link (the S4 rule: link-bound
    /// actions dim when unreachable).
    public var canSend: Bool {
        (phase == .staged || phase == .failed) && staged != nil && connection == .connected
    }

    public var fraction: Double { progress.fraction }

    /// "62%" — mono, beside the bar.
    public var percentLine: String { "\(Int((progress.fraction * 100).rounded()))%" }

    /// The `awaitingConfirm` headline — "Confirm on <device>" until the reboot
    /// starts, then "Installing update".
    public var awaitingTitle: String {
        sawDropSinceInstall ? "Installing update" : "Confirm on \(deviceName)"
    }

    /// The `awaitingConfirm` body: the one indispensable instruction before the
    /// reboot, then the rebooting status after (spec-mandated copy).
    public var awaitingMessage: String {
        sawDropSinceInstall
            ? "\(deviceName) is installing the update. It'll reconnect here when it's done."
            : "Confirm the update on \(deviceName). It restarts to install, then reconnects here."
    }

    /// The `done` line — the version now running.
    public var doneMessage: String {
        "\(deviceName) is running \(runningVersionLine)."
    }

    // MARK: Lifecycle

    /// Subscribe the link state and read the running version. Called from the
    /// view's `.task`, and **re-entrant across a `stop()`**: SwiftUI can cycle
    /// `onDisappear`/`onAppear` on a screen whose model persists (a future
    /// presentation pushed over S7, a scene re-attach), and a one-shot
    /// lifecycle would come back with a dead connection subscription —
    /// `connection`/`canSend` frozen. Idempotent while running (the `started`
    /// guard); `stop()` re-arms it. The transport's `state` replays the latest
    /// value per subscription (`AsyncMulticast`), so a re-subscribe sees the
    /// current link state, not just future edges.
    public func start() {
        guard !started else { return }
        started = true
        if let prestage, phase == .idle {
            stage(prestage)
            if autoSend, phase == .staged { send() }
        }
        stateTask = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                handleState(state)
            }
        }
        Task { [weak self, transport] in
            guard let info = try? await transport.deviceInfo() else { return }
            guard let self else { return }
            runningVersion = info.firmwareVersion
        }
    }

    private func handleState(_ state: ConnectionState) {
        connection = state
        let dropped = state == .outOfRange || state == .disconnected
        switch phase {
        case .transferring where dropped:
            // The link left the transfer stalled-but-restartable (Resume re-sends).
            if let handle, handle.currentOutcome == nil { phase = .interrupted }
        case .awaitingConfirm:
            if dropped { sawDropSinceInstall = true }
            if state == .connected, sawDropSinceInstall { checkInstalledVersion() }
        default:
            break
        }
    }

    /// A reconnect after the install reboot: re-read DIS; the staged version now
    /// running means the update landed.
    private func checkInstalledVersion() {
        Task { [weak self, transport] in
            guard let info = try? await transport.deviceInfo() else { return }
            guard let self else { return }
            runningVersion = info.firmwareVersion
            if let staged, info.firmwareVersion == staged.version, phase == .awaitingConfirm {
                phase = .done
            }
        }
    }

    // MARK: Import

    /// Read + validate a picked file. A good file becomes the staged update; a bad
    /// one sets `importError` (the alert) and changes no phase — the whole point
    /// is that a corrupt download fails here, not on the device.
    public func stageFile(at url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else {
            importError = "Couldn't read that file."
            return
        }
        stage(data)
    }

    /// Validate raw bytes as an update (the seam the tests + the picker share).
    public func stage(_ data: Data) {
        do {
            let firmware = try StagedFirmware.validate(data)
            staged = firmware
            failureMessage = nil
            progress = TransferProgress(bytesDone: 0, total: firmware.byteCount)
            phase = .staged
        } catch let error as FirmwareImageError {
            importError = Self.importMessage(for: error)
        } catch {
            importError = "Couldn't read that file."
        }
    }

    // MARK: Send + install

    /// Start (or retry) delivery: stream the container, then request the install.
    public func send() {
        guard phase == .staged || phase == .failed, let staged else { return }
        failureMessage = nil
        sawDropSinceInstall = false
        progress = TransferProgress(bytesDone: 0, total: staged.byteCount)
        phase = .transferring
        beginTransfer(staged)
    }

    private func beginTransfer(_ staged: StagedFirmware) {
        cancelTransferWatchers()
        let handle = transport.uploadFirmware(staged.container)
        self.handle = handle

        transferWatchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                if phase == .interrupted { phase = .transferring }  // moving again
            }
        })

        transferWatchers.append(Task { [weak self] in
            let outcome = await handle.outcome
            guard let self, !Task.isCancelled else { return }
            switch outcome {
            case .completed:
                await requestInstall()
            case .canceled:
                // Back to the staged file — still validated, ready to re-send.
                if phase != .done { phase = .staged }
            case .failed(let error):
                failureMessage = Self.transferFailureMessage(error, deviceName: deviceName)
                phase = .failed
            }
        })
    }

    /// The device committed `/UPDATE.BIN` — ask it to install (`installFw`). Only
    /// `accepted` opens the on-glass confirm flow; every other reply is a `.failed`
    /// phase with a plain sentence.
    private func requestInstall() async {
        do {
            let result = try await transport.installFirmware()
            switch result {
            case .accepted:
                sawDropSinceInstall = false
                phase = .awaitingConfirm
            case .busy, .noStaged, .rejected, .unsupported:
                failureMessage = Self.message(for: result, deviceName: deviceName)
                phase = .failed
            }
        } catch {
            failureMessage = Self.transferFailureMessage((error as? DeviceError) ?? .notConnected, deviceName: deviceName)
            phase = .failed
        }
    }

    /// Abort an in-flight or interrupted transfer.
    public func cancel() {
        handle?.cancel()
    }

    /// Restart a dropped transfer from scratch (uploads restart, not resume).
    public func resume() {
        guard phase == .interrupted else { return }
        handle?.resume()
        phase = .transferring
    }

    private func cancelTransferWatchers() {
        transferWatchers.forEach { $0.cancel() }
        transferWatchers.removeAll()
    }

    /// The S7 screen went off screen (`.onDisappear`). A still-unresolved send
    /// must not keep streaming headless behind the pop — cancel it (the same
    /// rule as `UploadSheetModel.sheetDismissed`) — and release the ledger
    /// claim here, since a `@MainActor` `deinit` can't touch the actor-isolated
    /// `TransferActivity`. The counterpart of `start()`: re-arms `started`, so
    /// a later `onAppear`'s `start()` re-subscribes instead of silently doing
    /// nothing on a frozen model.
    public func stop() {
        started = false
        stateTask?.cancel()
        stateTask = nil
        // Watchers first, then the handle: the cancel resolves the outcome to
        // `.canceled`, and with its watcher already gone the phase settle below
        // is authoritative — no race over who writes the post-cancel phase.
        cancelTransferWatchers()
        if let handle, handle.currentOutcome == nil { handle.cancel() }
        // Same landing as a watched cancel: back to the staged file, still
        // validated and ready to re-send — not a frozen `.transferring` on a
        // model that may reappear. The `didSet` releases the ledger claim.
        if phase == .transferring || phase == .interrupted { phase = .staged }
        // Backstop (idempotent — the `didSet` normally already released).
        if let token = activityToken {
            activityToken = nil
            activity?.end(token)
        }
    }

    deinit {
        stateTask?.cancel()
        transferWatchers.forEach { $0.cancel() }
    }

    // MARK: Copy tables

    /// The mapped `installFw` reply sentence (spec §4.4). `nil` for `accepted`
    /// (that opens the confirm flow, no failure copy).
    static func message(for result: FirmwareInstallResult, deviceName: String) -> String? {
        switch result {
        case .accepted:
            return nil
        case .busy:
            return "Finish or discard the current ride on \(deviceName) first, then send it again."
        case .noStaged:
            return "\(deviceName) doesn't see the update — send it again."
        case .rejected:
            return "\(deviceName) rejected the update."
        case .unsupported:
            return "\(deviceName) can't be updated over Bluetooth."
        }
    }

    /// A transfer/link failure sentence — storage-full is spelled out; everything
    /// else keeps the "device didn't answer" framing.
    static func transferFailureMessage(_ error: DeviceError, deviceName: String) -> String {
        switch error {
        case .crcMismatch:
            return "The update didn't arrive intact. Send it again."
        default:
            return "\(deviceName) didn't answer. Check that it's awake and nearby, then send it again."
        }
    }

    /// The picker's rejection sentence for a file that isn't a usable update.
    static func importMessage(for error: FirmwareImageError) -> String {
        switch error {
        case .tooSmall, .notOBCU:
            return "That file isn't an OpenBikeComputer firmware update."
        case .oversize:
            return "That update is too large for this device."
        case .truncated:
            return "That update file looks incomplete. Download it again, then reimport."
        case .imageCRCMismatch:
            return "That update file is corrupt. Download it again, then reimport."
        }
    }

    /// "v1.2.0" — prefix a bare version with "v"; pass through one that has it.
    private static func versioned(_ version: String?) -> String? {
        guard let version, !version.isEmpty else { return nil }
        return version.hasPrefix("v") ? version : "v\(version)"
    }
}
