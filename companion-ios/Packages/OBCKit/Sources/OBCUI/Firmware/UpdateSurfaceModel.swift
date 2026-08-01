import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The launch surface (#773 U5): the thing that decides, when the app comes to the front, whether
/// the rider should be told about a published firmware update they haven't been told about.
///
/// It owns no rules. Every "should we?" goes to ``UpdateSurfaceRunner`` (and through it
/// ``UpdateSurfacePolicy``), which is shared verbatim with the background refresh — so the sheet
/// and the notification can never drift into disagreeing about what's worth interrupting a rider
/// for. What lives here is the *lifecycle*: when to ask, what to remember about the device, and
/// what a dismiss means.
///
/// **It is not a poller.** Becoming active runs the policy, and the policy answers from U4's 6-hour
/// cache — so foregrounding the app ten times in a minute makes at most one network request, and
/// usually none.
///
/// **The permission moment.** Notification authorization is requested the first time this sheet is
/// presented, and nowhere else — never at launch. At that moment the rider is looking at the exact
/// thing a future notification would be about, so the ask has a subject; before it, there is
/// nothing to notify about and the prompt would be noise. It asks *provisionally* (the adapter's
/// choice), so even this moment cannot interrupt with a system alert. A denial is silent: the
/// launch sheet keeps working, which is the surface that doesn't need permission.
@MainActor @Observable
public final class UpdateSurfaceModel {
    /// One presented offer. `Identifiable` for `.sheet(item:)`; the version is the identity, so a
    /// re-decide that lands on the same release doesn't re-present.
    public struct PendingUpdate: Identifiable, Equatable {
        public var id: String { release.version }
        public let release: FirmwareRelease
        public let deviceName: String
    }

    /// The offer on screen, or `nil`. Set only by the policy.
    public private(set) var pending: PendingUpdate?

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    /// The shared decision path. `nil` in wiring that doesn't want a launch check (previews, the
    /// screens' own tests) — which leaves the app exactly the U4 app.
    @ObservationIgnored private let runner: UpdateSurfaceRunner?
    /// The notification adapter, asked for permission at the moment below. `nil` where there is no
    /// notification center to talk to (previews, `swift test`).
    @ObservationIgnored private let notifier: (any UpdateNotifying)?
    @ObservationIgnored private var stateTask: Task<Void, Never>?
    @ObservationIgnored private var checkTask: Task<Void, Never>?
    @ObservationIgnored private var started = false
    /// The device the current offer is about, so a dismiss writes the ledger under the right key.
    @ObservationIgnored private var pendingDevice: LastSeenDevice?

    public init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        runner: UpdateSurfaceRunner? = nil,
        notifier: (any UpdateNotifying)? = nil
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.runner = runner
        self.notifier = notifier
    }

    // MARK: Lifecycle

    /// Watch the link so the running version is remembered while it *can* be read — a wake hours
    /// later, with the device in a pannier, has to compare against something. Idempotent.
    public func start() {
        guard !started, runner != nil else { return }
        started = true
        stateTask = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                guard state == .connected else { continue }
                await rememberDevice()
                // The link usually arrives a second or two after the app does, so the launch check
                // that ran on `.active` had only the persisted version (or none, on a first run).
                // Re-decide now that the live one is in hand — off the cache, so this costs
                // nothing when the answer is already known.
                appBecameActive()
            }
        }
    }

    /// The app came to the front. Runs the policy; presents a sheet only if it says to.
    public func appBecameActive() {
        guard let runner, pending == nil, checkTask == nil else { return }
        checkTask = Task { [weak self, runner] in
            // A live read beats the remembered one — and it refreshes the memory. `?? nil` flattens
            // the optional chain: no model and no link land in the same place, the persisted record.
            let live = await self?.rememberDevice() ?? nil
            // Capture the fallback once. A different device can connect while the network request
            // is in flight; the offer must remain labelled and ledgered for the device whose
            // running version the policy actually evaluated.
            let target = runner.device(live)
            let release = await runner.run(device: target)
            guard let self else { return }
            checkTask = nil
            guard !Task.isCancelled, pending == nil, let release else { return }
            present(release, device: target)
        }
    }

    /// Read DIS and persist it as the last-seen device. Returns what it read, or `nil` when the
    /// link isn't up — in which case the caller falls back to whatever was remembered.
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

    /// The permission moment — here, once, and only with an offer on screen (see the type's note).
    private func askForNotificationPermissionOnce() {
        guard let runner, let notifier, !runner.didAskNotificationPermission else { return }
        runner.markAskedNotificationPermission()
        Task { await notifier.requestAuthorization() }
    }

    // MARK: The two answers

    /// "View" — the rider is acting on it, which is also an answer. Navigation is the host's: this
    /// model knows nothing about the stack, so the same two answers work under any presentation.
    public func viewUpdate() {
        answer()
    }

    /// "Not now" (and a swipe-down, which the presentation binds here too). Also an answer: this
    /// version has been put to the rider and won't be again. A *newer* one is a new question.
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
