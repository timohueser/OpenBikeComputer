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
///
/// ## The published-release check (#773 U4)
///
/// Alongside the Files picker the model can ask ``UpdateChecker`` what the newest
/// published build is and, if it's newer than DIS 0x2A26 says, download it and
/// feed it **the same `stage(_:)` path the picker feeds**. The download is proved
/// against the manifest's byte count and SHA-256 before it gets anywhere near the
/// link, and the device-side confirm is untouched: an offered update still travels
/// as a `fwImage` and still installs only on a physical press. Nothing here decides
/// *when* to check on its own — no timers, no launch sheet, no background task;
/// that is #773's U5, which drives this same surface from outside.
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

    // MARK: The published-release check (U4)

    /// Where the update check is: `.checking` only while a fetch is in flight, and
    /// `.failed` only for a check the *rider* asked for — an automatic on-appear
    /// check that can't reach the network says nothing (there is nothing to act on
    /// and a plane has no update problem).
    public enum CheckState: Equatable {
        case idle
        case checking
        case failed(String)
    }

    /// The download+verify leg of "Download & Install".
    public enum DownloadState: Equatable {
        case idle
        case downloading
        case failed(String)
    }

    /// The newest published release, from the cache on open and from the network
    /// after a check. `nil` means nothing is published (or nothing is known yet).
    public private(set) var latestRelease: FirmwareRelease?
    /// When the cached answer was taken.
    public private(set) var lastCheckedAt: Date?
    public private(set) var checkState: CheckState = .idle
    public private(set) var downloadState: DownloadState = .idle

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
    /// The published-release check (U4) — `nil` in wiring that doesn't want one
    /// (previews, the transfer-state tests), which leaves the screen exactly the
    /// Files-picker-only S7 it was.
    @ObservationIgnored private let updateChecker: UpdateChecker?
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var checkTask: Task<Void, Never>?
    @ObservationIgnored private var downloadTask: Task<Void, Never>?
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
        updateChecker: UpdateChecker? = nil,
        prestage: Data? = nil,
        autoSend: Bool = false
    ) {
        self.transport = transport
        self.deviceName = deviceName
        self.activity = activity
        self.updateChecker = updateChecker
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

    /// The link isn't dropped. The tick watcher reads this to tell a genuine
    /// resume tick from a stale pre-drop one: ticks and link states arrive on
    /// two independent streams, so a backlogged tick can be delivered *after*
    /// the drop it preceded.
    private var linkUp: Bool {
        connection != .outOfRange && connection != .disconnected
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

    // MARK: Derived copy — the update check

    /// The check's answer for *this* device: the published version against what
    /// DIS reports. Derived, never stored, so it re-reads the moment either half
    /// lands.
    public var updateStatus: FirmwareUpdateStatus {
        FirmwareVersion.updateStatus(running: runningVersion, latest: latestRelease?.version)
    }

    /// The screen has an answer worth showing. Until DIS 0x2A26 lands there is no
    /// running version to compare against, and ``updateStatus`` reads `.unknown`
    /// for want of one — which must not be mistaken for "this is a dev build".
    public var hasUpdateAnswer: Bool { runningVersion != nil }

    /// "v1.4.0" — the published version, empty when nothing is published.
    public var latestVersionLine: String { Self.versioned(latestRelease?.version) ?? "" }

    /// "854 KB" — the published container's size.
    public var latestSizeLine: String {
        guard let latestRelease else { return "" }
        return ByteCountFormatter.string(fromByteCount: Int64(latestRelease.bytes), countStyle: .file)
    }

    /// The release-notes link, when the manifest points at one this app can open.
    public var releaseNotesURL: URL? { latestRelease?.notesURL }

    /// "Checked 5 minutes ago" — the check row's trailing value; "Never" before the
    /// first one.
    public var lastCheckedLine: String {
        guard let lastCheckedAt else { return "Never" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return formatter.localizedString(for: lastCheckedAt, relativeTo: Date())
    }

    /// A running version that isn't a release version — #773's locked refusal. The
    /// manual Files path stays available; only the *automatic* offer is paused.
    public var developmentBuild: Bool { hasUpdateAnswer && updateStatus == .unknown }

    /// Offer the download only for a genuinely newer published build, and never
    /// while one is already staged or moving.
    public var canDownloadUpdate: Bool {
        updateStatus == .available && downloadState != .downloading
            && (phase == .idle || phase == .staged || phase == .failed)
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
        checkForUpdate()
    }

    private func handleState(_ state: ConnectionState) {
        connection = state
        let dropped = !linkUp
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

    /// Validate raw bytes as an update (the seam the tests, the picker and the
    /// U4 download all share). `true` when *these* bytes became the staged update
    /// — the download path needs that to be sure it isn't sending an older file
    /// that happened to be staged already.
    @discardableResult
    public func stage(_ data: Data) -> Bool {
        do {
            let firmware = try StagedFirmware.validate(data)
            staged = firmware
            failureMessage = nil
            progress = TransferProgress(bytesDone: 0, total: firmware.byteCount)
            phase = .staged
            return true
        } catch let error as FirmwareImageError {
            importError = Self.importMessage(for: error)
        } catch {
            importError = "Couldn't read that file."
        }
        return false
    }

    // MARK: The update check (U4)

    /// This wiring has a checker at all (previews and the transfer tests don't).
    public var supportsUpdateCheck: Bool { updateChecker != nil }

    /// The dev opt-in: also consider the pre-release channel, and offer whichever
    /// of the two is newer. Off by default, and surfaced only in the Debug
    /// developer section — a rider never sees it.
    public var includePrereleases: Bool { updateChecker?.includePrereleases ?? false }

    /// Flip the pre-release opt-in and re-ask straight away: the channel changed,
    /// so the cached answer is about a question nobody asked.
    public func setIncludePrereleases(_ include: Bool) {
        updateChecker?.setIncludePrereleases(include)
        checkForUpdate(manual: true)
    }

    /// Answer from the cache immediately, then re-ask the network if that answer is
    /// stale (or the rider pulled to refresh, `manual: true`).
    ///
    /// Called from `start()`, so opening the screen is the check trigger. The
    /// cached answer is applied first and unconditionally: a screen that opens
    /// offline still shows what it knew, and a fetch that fails never erases it.
    public func checkForUpdate(manual: Bool = false) {
        guard let updateChecker else { return }
        if let cached = updateChecker.cachedCheck() {
            apply(cached)
            if !manual, updateChecker.isFresh(cached) { return }
        }
        guard checkState != .checking else { return }
        checkState = .checking
        checkTask?.cancel()
        checkTask = Task { [weak self, updateChecker] in
            do {
                let record = try await updateChecker.check()
                guard let self, !Task.isCancelled else { return }
                apply(record)
                checkState = .idle
            } catch is CancellationError {
                return
            } catch {
                guard let self, !Task.isCancelled else { return }
                // A manual check that fails owes the rider a sentence; an automatic
                // one owes them silence.
                checkState = manual ? .failed(Self.checkFailureMessage(error)) : .idle
            }
        }
    }

    private func apply(_ record: UpdateCheckRecord) {
        latestRelease = record.release
        lastCheckedAt = record.checkedAt
    }

    /// Download the published container, prove it against the manifest, and hand it
    /// to the **same staging path the Files picker uses** — then send it, if the
    /// link is up. Nothing is installed by any of this: the device still shows its
    /// confirm card and still waits for a physical press.
    ///
    /// A download that doesn't match the manifest's byte count or SHA-256 is thrown
    /// away with a plain sentence and never reaches ``stage(_:)``, so a corrupt or
    /// swapped file dies on the phone — the same rule as a bad file from the picker.
    public func downloadUpdate() {
        guard let updateChecker, let release = latestRelease, canDownloadUpdate else { return }
        downloadState = .downloading
        downloadTask?.cancel()
        downloadTask = Task { [weak self, updateChecker] in
            do {
                let data = try await updateChecker.download(release)
                guard let self, !Task.isCancelled else { return }
                downloadState = .idle
                // A verified container that stages cleanly goes straight out to the
                // device — "Download & Install" shouldn't need a second tap. It
                // still can't install anything: the on-glass confirm is the gate.
                // Keyed on *this* stage succeeding, so a rejected download can
                // never send whatever was staged before it.
                if stage(data), canSend { send() }
            } catch is CancellationError {
                return
            } catch {
                guard let self, !Task.isCancelled else { return }
                downloadState = .failed(Self.downloadFailureMessage(error))
            }
        }
    }

    /// Dismiss the download/check failure line (its "OK").
    public func clearUpdateError() {
        if case .failed = downloadState { downloadState = .idle }
        if case .failed = checkState { checkState = .idle }
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

        // Progress ticks. A tick is also the proof a resume is moving again —
        // but only while the link is up: a stale pre-drop tick delivered after
        // the drop event must not flip the sheet back to `.transferring`
        // (hiding Resume) for a transfer whose link is already gone.
        transferWatchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                if phase == .interrupted, linkUp { phase = .transferring }  // moving again
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
        // The check + download are screen-scoped too: a popped screen has nobody
        // to tell, and `start()` re-runs the check from the cache anyway. A
        // download killed mid-flight stages nothing — the bytes are re-fetched.
        checkTask?.cancel()
        checkTask = nil
        downloadTask?.cancel()
        downloadTask = nil
        if checkState == .checking { checkState = .idle }
        if downloadState == .downloading { downloadState = .idle }
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
        checkTask?.cancel()
        downloadTask?.cancel()
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

    /// A failed *manual* check. The manifest errors say what's wrong at the
    /// publishing end (they're loud on purpose — #773 U4); everything else is the
    /// network.
    static func checkFailureMessage(_ error: any Error) -> String {
        guard let manifest = error as? FirmwareManifestError else {
            return "Couldn't check for updates. Check your connection, then try again."
        }
        switch manifest {
        case .httpStatus(let status):
            return "The update server answered with an error (HTTP \(status)). Try again later."
        default:
            return "The published update information is unreadable, so nothing is being offered. "
                + "This is a problem at our end — try again later."
        }
    }

    /// A failed download. A file that doesn't match the manifest is a corrupt or
    /// wrong file, and saying so plainly matters more than the distinction between
    /// a bad size and a bad digest — neither one reaches the device.
    static func downloadFailureMessage(_ error: any Error) -> String {
        switch error {
        case is FirmwareDownloadError:
            return "The download didn't match the published update, so it wasn't sent. Try again."
        default:
            return "Couldn't download the update. Check your connection, then try again."
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
