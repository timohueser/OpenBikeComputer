import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Decides at foreground whether to offer a published firmware update.
/// The rules live in `UpdateSurfaceRunner`, shared with the background refresh, so
/// the sheet and the notification cannot disagree. It does not poll: the policy
/// answers from a cache, so repeated foregrounding costs at most one request.
/// Notification authorization is requested the first time this sheet is presented.
@MainActor @Observable
public final class UpdateSurfaceModel {
    /// One presented offer. The version is the identity, so a re-decide on the same
    /// release does not present again.
    public struct PendingUpdate: Identifiable, Equatable {
        public var id: String { release.version }
        public let release: FirmwareRelease
        public let deviceName: String
    }

    public private(set) var pending: PendingUpdate?

    private let transport: any DeviceLink
    private let bondStore: any BondStore
    /// The shared decision path. `nil` disables the launch check (previews, tests).
    @ObservationIgnored private let runner: UpdateSurfaceRunner?
    /// `nil` where there is no notification center to talk to (previews, tests).
    @ObservationIgnored private let notifier: (any UpdateNotifying)?
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var checkTask: Task<Void, Never>?
    @ObservationIgnored private var started = false
    /// The device the offer is about, so a dismiss writes the ledger under the right key.
    @ObservationIgnored private var pendingDevice: LastSeenDevice?

    public init(
        transport: any DeviceLink,
        bondStore: any BondStore,
        runner: UpdateSurfaceRunner? = nil,
        notifier: (any UpdateNotifying)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.runner = runner
        self.notifier = notifier
    }

    /// Watch the link and remember the running version while it can be read. Idempotent.
    public func start() {
        guard !started, runner != nil else { return }
        started = true
        stateTask = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                guard state == .connected else { continue }
                await rememberDevice()
                // The link arrives after the app does, so the check on `.active` had
                // only the persisted version. Re-decide with the live one; the cache
                // makes a known answer free.
                appBecameActive()
            }
        }
    }

    /// The app came to the front. Runs the policy; presents a sheet only if it says to.
    public func appBecameActive() {
        guard let runner, pending == nil, checkTask == nil else { return }
        checkTask = Task { [weak self, runner] in
            // A live read beats the remembered one and refreshes it. `?? nil` flattens
            // the optional chain: no model and no link both land on the persisted record.
            let live = await self?.rememberDevice() ?? nil
            // Capture the fallback once: another device can connect while the request
            // is in flight, and the offer must stay with the evaluated device.
            let target = runner.device(live)
            let release = await runner.run(device: target)
            guard let self else { return }
            checkTask = nil
            guard !Task.isCancelled, pending == nil, let release else { return }
            present(release, device: target)
        }
    }

    /// Read the device info and persist it as last-seen. `nil` when the link is down.
    @discardableResult
    private func rememberDevice() async -> LastSeenDevice? {
        guard let runner, let info = try? await transport.deviceInfo() else { return nil }
        let device = LastSeenDevice(
            serial: info.serial, firmwareVersion: info.firmwareVersion, seenAt: Date()
        )
        runner.remember(device)
        return device
    }

    private func present(_ release: FirmwareRelease, device: LastSeenDevice?) {
        pendingDevice = device
        pending = PendingUpdate(
            release: release,
            deviceName: bondStore.load()?.deviceName ?? "your bike computer"
        )
        askForNotificationPermissionOnce()
    }

    private func askForNotificationPermissionOnce() {
        guard let runner, let notifier, !runner.didAskNotificationPermission else { return }
        runner.markAskedNotificationPermission()
        Task { await notifier.requestAuthorization() }
    }

    /// "View": acting on the offer is also an answer. Navigation belongs to the host.
    public func viewUpdate() {
        answer()
    }

    /// "Not now", or a swipe-down. This version is not offered again; a newer one is.
    public func dismiss() {
        answer()
    }

    private func answer() {
        if let version = pending?.release.version {
            runner?.recordAnswered(version: version, device: pendingDevice)
        }
        pending = nil
        pendingDevice = nil
    }

    deinit {
        stateTask?.cancel()
        checkTask?.cancel()
    }
}
