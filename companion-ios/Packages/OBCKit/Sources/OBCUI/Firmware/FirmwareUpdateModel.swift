import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the firmware-update screen: import an `UPDATE.BIN`, stream it to the device, then
/// request the on-glass install.
///
/// The transfer restarts whole: a drop re-sends from scratch and never resumes. After the device
/// commits the image the model requests the install; on `accepted` it waits, because the rider
/// confirms on the device, which reboots to install and drops the link. "Done" is the normal
/// reconnect with the staged version now reported. There is no progress for the flash phase: the
/// device is off-link in the bootloader.
///
/// The model can also ask ``UpdateChecker`` for the newest published build and feed it the same
/// `stage(_:)` path the picker feeds. A download is proved against the manifest's byte count and
/// SHA-256 before it gets anywhere near the link, and the device-side confirm is untouched.
/// Nothing here decides when to check: no timers, no launch sheet, no background task.
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
        // The in-flight ledger claim, held exactly while a send is moving bytes. Mirroring it on
        // the phase transition covers every exit uniformly. A stalled `.interrupted` transfer
        // drops the claim on purpose: the background drain must not wait on a transfer whose
        // link is already gone, and the screen need not stay awake for a send that is stalled.
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
    /// The device's running firmware version, nil until it lands or while the link is down.
    public private(set) var runningVersion: String?
    public private(set) var staged: StagedFirmware?
    public private(set) var connection: ConnectionState = .connecting
    /// A picked file that is not a usable update, surfaced as an alert and never a phase.
    /// Settable, so the alert's dismissal clears it.
    public var importError: String?
    /// The failure sentence for a `.failed` phase. Nil in every other phase.
    public private(set) var failureMessage: String?

    // MARK: The published-release check

    /// Where the update check is. `.failed` only for a check the rider asked for: an automatic
    /// check that cannot reach the network says nothing, because there is nothing to act on.
    public enum CheckState: Equatable {
        case idle
        case checking
        case failed(String)
    }

    /// The download and verify leg of "Download & Install".
    public enum DownloadState: Equatable {
        case idle
        case downloading
        case failed(String)
    }

    /// The newest published release, from the cache on open and from the network after a check.
    /// Nil means nothing is published, or nothing is known yet.
    public private(set) var latestRelease: FirmwareRelease?
    public private(set) var lastCheckedAt: Date?
    public private(set) var checkState: CheckState = .idle
    public private(set) var downloadState: DownloadState = .idle

    // MARK: Fixed facts

    public let deviceName: String

    // MARK: Wiring

    private let transport: any DeviceLink & DeviceUpdates
    /// The foreground-only policy's in-flight ledger, shared with the upload sheet and the
    /// ride-sync coordinator. A firmware send claims a token while transferring, so it is drained
    /// and not dropped across a background transition. Nil in previews and tests.
    @ObservationIgnored private let activity: TransferActivity?
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    /// Nil in wiring that does not want an update check, which leaves the screen the Files
    /// picker alone.
    @ObservationIgnored private let updateChecker: UpdateChecker?
    @ObservationIgnored private var handle: TransferHandle?
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var checkTask: Task<Void, Never>?
    @ObservationIgnored private var downloadTask: Task<Void, Never>?
    @ObservationIgnored private var transferWatchers: [Task<Void, Never>] = []
    @ObservationIgnored private var started = false
    /// Set once the link drops after an accepted install: the reboot is under way, so the copy
    /// switches from "confirm on the device" to "installing".
    @ObservationIgnored private var sawDropSinceInstall = false

    /// A pre-staged update for automation and previews, validated and staged on `start()`,
    /// because the Files picker cannot be driven from automation.
    @ObservationIgnored private let prestage: Data?
    /// Fire Send once the pre-staged file is validated, so a demo run walks the whole flow.
    @ObservationIgnored private let autoSend: Bool

    public init(
        transport: any DeviceLink & DeviceUpdates,
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

    public var runningVersionLine: String { Self.versioned(runningVersion) ?? "—" }

    public var stagedVersionLine: String { Self.versioned(staged?.version) ?? "" }

    public var stagedSizeLine: String {
        guard let staged else { return "" }
        return ByteCountFormatter.string(fromByteCount: Int64(staged.byteCount), countStyle: .file)
    }

    /// The staged version already matches what is running, so sending it again is pointless.
    public var stagedMatchesRunning: Bool {
        guard let staged, let runningVersion else { return false }
        return staged.version == runningVersion
    }

    /// Sending needs a validated file and a live link.
    public var canSend: Bool {
        (phase == .staged || phase == .failed) && staged != nil && connection == .connected
    }

    /// The link is not dropped. The tick watcher reads this to tell a genuine resume tick from a
    /// stale pre-drop one: ticks and link states arrive on two independent streams, so a
    /// backlogged tick can be delivered after the drop it preceded.
    private var linkUp: Bool {
        connection != .outOfRange && connection != .disconnected
    }

    public var fraction: Double { progress.fraction }

    public var percentLine: String { "\(Int((progress.fraction * 100).rounded()))%" }

    public var awaitingTitle: String {
        sawDropSinceInstall ? "Installing update" : "Confirm on \(deviceName)"
    }

    public var awaitingMessage: String {
        sawDropSinceInstall
            ? "\(deviceName) is installing the update. It'll reconnect here when it's done."
            : "Confirm the update on \(deviceName). It restarts to install, then reconnects here."
    }

    public var doneMessage: String {
        "\(deviceName) is running \(runningVersionLine)."
    }

    // MARK: Derived copy — the update check

    /// The published version against what the device reports. Derived, never stored, so it
    /// re-reads the moment either half lands.
    public var updateStatus: FirmwareUpdateStatus {
        FirmwareVersion.updateStatus(running: runningVersion, latest: latestRelease?.version)
    }

    /// The screen has an answer worth showing. Until the running version lands, ``updateStatus``
    /// reads `.unknown` for want of one, which must not be mistaken for a development build.
    public var hasUpdateAnswer: Bool { runningVersion != nil }

    public var latestVersionLine: String { Self.versioned(latestRelease?.version) ?? "" }

    public var latestSizeLine: String {
        guard let latestRelease else { return "" }
        return ByteCountFormatter.string(fromByteCount: Int64(latestRelease.bytes), countStyle: .file)
    }

    public var releaseNotesURL: URL? { latestRelease?.notesURL }

    public var lastCheckedLine: String {
        guard let lastCheckedAt else { return "Never" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return formatter.localizedString(for: lastCheckedAt, relativeTo: Date())
    }

    /// A running version that is not a release version. The manual Files path stays available;
    /// only the automatic offer is paused.
    public var developmentBuild: Bool { hasUpdateAnswer && updateStatus == .unknown }

    /// Offer the download only for a genuinely newer published build, and never while one is
    /// already staged or moving.
    public var canDownloadUpdate: Bool {
        updateStatus == .available && downloadState != .downloading
            && (phase == .idle || phase == .staged || phase == .failed)
    }

    // MARK: Lifecycle

    /// Subscribe the link state and read the running version. Re-entrant across a `stop()`:
    /// SwiftUI can cycle `onDisappear` and `onAppear` on a screen whose model persists, and a
    /// one-shot lifecycle would come back with a dead connection subscription. The transport's
    /// `state` replays its latest value per subscription, so a re-subscribe sees the current
    /// link state, not only future edges.
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
            // The link left the transfer stalled but restartable; Resume re-sends.
            if let handle, handle.currentOutcome == nil { phase = .interrupted }
        case .awaitingConfirm:
            if dropped { sawDropSinceInstall = true }
            if state == .connected, sawDropSinceInstall { checkInstalledVersion() }
        default:
            break
        }
    }

    /// A reconnect after the install reboot: re-read the device info. The staged version now
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

    /// Read and validate a picked file. A bad one sets `importError` and changes no phase: the
    /// whole point is that a corrupt download fails here, not on the device.
    public func stageFile(at url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else {
            importError = "Couldn't read that file."
            return
        }
        stage(data)
    }

    /// Validate raw bytes as an update, the seam the tests, the picker and the download share.
    /// True when these bytes became the staged update, which the download path needs so that it
    /// cannot send an older file that happened to be staged already.
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

    // MARK: The update check

    /// Whether this wiring has a checker at all.
    public var supportsUpdateCheck: Bool { updateChecker != nil }

    /// The developer opt-in: also consider the pre-release channel and offer whichever of the two
    /// is newer. Off by default, and surfaced only in the Debug developer section.
    public var includePrereleases: Bool { updateChecker?.includePrereleases ?? false }

    /// Flip the pre-release opt-in and re-ask straight away: the channel changed, so the cached
    public func setIncludePrereleases(_ include: Bool) {
        updateChecker?.setIncludePrereleases(include)
        checkForUpdate(manual: true)
    }

    /// Answer from the cache immediately, then re-ask the network if that answer is stale, or the
    /// rider pulled to refresh. Called from `start()`, so opening the screen is the trigger. The
    /// cached answer is applied first and unconditionally: a screen that opens offline still shows
    /// what it knew, and a fetch that fails never erases it.
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
                // A manual check that fails owes the rider a sentence; an automatic one silence.
                checkState = manual ? .failed(Self.checkFailureMessage(error)) : .idle
            }
        }
    }

    private func apply(_ record: UpdateCheckRecord) {
        latestRelease = record.release
        lastCheckedAt = record.checkedAt
    }

    /// Download the published container, prove it against the manifest, and hand it to the same
    /// staging path the Files picker uses, then send it if the link is up. A download that does
    /// not match the manifest's byte count or SHA-256 is thrown away and never reaches
    /// ``stage(_:)``, so a corrupt file dies on the phone. Nothing here installs anything.
    public func downloadUpdate() {
        guard let updateChecker, let release = latestRelease, canDownloadUpdate else { return }
        downloadState = .downloading
        downloadTask?.cancel()
        downloadTask = Task { [weak self, updateChecker] in
            do {
                let data = try await updateChecker.download(release)
                guard let self, !Task.isCancelled else { return }
                downloadState = .idle
                // A verified container that stages cleanly goes straight out to the device, so
                // "Download & Install" needs no second tap. Keyed on this stage succeeding, so a
                // rejected download can never send whatever was staged before it.
                if stage(data), canSend { send() }
            } catch is CancellationError {
                return
            } catch {
                guard let self, !Task.isCancelled else { return }
                downloadState = .failed(Self.downloadFailureMessage(error))
            }
        }
    }

    public func clearUpdateError() {
        if case .failed = downloadState { downloadState = .idle }
        if case .failed = checkState { checkState = .idle }
    }

    // MARK: Send + install

    /// Start, or retry, delivery: stream the container, then request the install.
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

        // A tick is also the proof a resume is moving again, but only while the link is up: a
        // stale pre-drop tick delivered after the drop must not flip the sheet back to
        // `.transferring` and hide Resume for a transfer whose link is already gone.
        transferWatchers.append(Task { [weak self] in
            for await tick in handle.progress {
                guard let self else { return }
                progress = tick
                if phase == .interrupted, linkUp { phase = .transferring }
            }
        })

        transferWatchers.append(Task { [weak self] in
            let outcome = await handle.outcome
            guard let self, !Task.isCancelled else { return }
            switch outcome {
            case .completed:
                await requestInstall()
            case .canceled:
                // Back to the staged file, still validated and ready to re-send.
                if phase != .done { phase = .staged }
            case .failed(let error):
                failureMessage = Self.transferFailureMessage(error, deviceName: deviceName)
                phase = .failed
            }
        })
    }

    /// The device committed the image: ask it to install. Only `accepted` opens the on-glass
    /// confirm flow; every other reply is a `.failed` phase with a plain sentence.
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

    public func cancel() {
        handle?.cancel()
    }

    /// Restart a dropped transfer from scratch: uploads restart, they do not resume.
    public func resume() {
        guard phase == .interrupted else { return }
        handle?.resume()
        phase = .transferring
    }

    private func cancelTransferWatchers() {
        transferWatchers.forEach { $0.cancel() }
        transferWatchers.removeAll()
    }

    /// The screen went off screen. A still-unresolved send must not keep streaming headless
    /// behind the pop, so cancel it, and release the ledger claim here, because a `@MainActor`
    /// `deinit` cannot touch the actor-isolated `TransferActivity`. The counterpart of `start()`:
    /// it re-arms `started`, so a later `start()` re-subscribes.
    public func stop() {
        started = false
        stateTask?.cancel()
        stateTask = nil
        // The check and the download are screen-scoped too: a popped screen has nobody to tell,
        // and `start()` re-runs the check from the cache anyway. A killed download stages nothing.
        checkTask?.cancel()
        checkTask = nil
        downloadTask?.cancel()
        downloadTask = nil
        if checkState == .checking { checkState = .idle }
        if downloadState == .downloading { downloadState = .idle }
        // Watchers first, then the handle: the cancel resolves the outcome to `.canceled`, and
        // with its watcher already gone the phase settle below is authoritative.
        cancelTransferWatchers()
        if let handle, handle.currentOutcome == nil { handle.cancel() }
        // Same landing as a watched cancel: back to the staged file, still validated and ready to
        // re-send, not a frozen `.transferring` on a model that may reappear.
        if phase == .transferring || phase == .interrupted { phase = .staged }
        // Backstop; the `didSet` normally released already.
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

    /// The mapped install-reply sentence. Nil for `accepted`, which opens the confirm flow.
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

    static func transferFailureMessage(_ error: DeviceError, deviceName: String) -> String {
        switch error {
        case .crcMismatch:
            return "The update didn't arrive intact. Send it again."
        default:
            return "\(deviceName) didn't answer. Check that it's awake and nearby, then send it again."
        }
    }

    /// A failed manual check. The manifest errors say what is wrong at the publishing end;
    /// everything else is the network.
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

    /// A failed download. A file that does not match the manifest is a corrupt or wrong file, and
    /// saying so plainly matters more than the difference between a bad size and a bad digest.
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
        case .unsigned:
            // Intact, but not ours: a different problem with a different fix, so it gets its own
            // sentence instead of being folded into "corrupt".
            return "That update file isn't signed for this device. Use an official release download."
        }
    }

    private static func versioned(_ version: String?) -> String? {
        guard let version, !version.isEmpty else { return nil }
        return version.hasPrefix("v") ? version : "v\(version)"
    }
}
