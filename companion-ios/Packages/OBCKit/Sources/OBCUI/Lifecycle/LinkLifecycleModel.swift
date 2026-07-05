import SwiftUI
import Observation
import OBCDomain
import OBCTransport

/// The foreground-only link policy (#459, epic #458): the app holds the BLE
/// link **only while foregrounded** — holding it in the background burns
/// battery on both ends for no user-visible benefit.
///
/// ```
/// foreground ── .background ──► draining ── ledger empty ⊻ grace expired ──► suspended
///     ▲                            │ .active (came right back — link never dropped)
///     │                            ▼
///     └────── .active + resumeLink() ◄──────────────────────────── suspended
/// ```
///
/// The rules (locked on the epic):
/// - A real `.background` transition suspends the link; `.inactive` flickers
///   (notification shade, app switcher) never churn it — only the
///   `.inactive` → `.background` distinction, no timers.
/// - **An in-flight transfer/sync is never dropped**: while the
///   `TransferActivity` ledger is non-empty, the suspend drains under a system
///   grace window (`BackgroundTaskRunner`) and disconnects after. An idle link
///   suspends promptly.
/// - The suspend goes through `DeviceTransport.suspendLink()`, whose contract
///   is drop **and pause the transport's reconnect loop** — otherwise the loop
///   fights the intentional disconnect and re-raises the link.
/// - Foreground re-raises via `resumeLink()` — the existing bonded
///   silent-reconnect path — but only when a link existed at suspend time (a
///   never-paired session must not start scanning because the user checked a
///   text). The reconnect's `.disconnected → .connected` edge is what triggers
///   `MainScreenModel`'s existing reload, truing up anything that changed on
///   the device while the app was away.
///
/// Depends only on `DeviceTransport` + the two seams (the golden rule) — the
/// host view feeds it raw `ScenePhase` changes; UIKit stays in the app target.
@MainActor @Observable
public final class LinkLifecycleModel {
    /// Where the policy currently stands. Exposed for the host/tests; nothing
    /// user-facing renders it (L1 ships no new copy).
    public enum LinkPhase: Equatable, Sendable {
        /// Foregrounded — the transport owns the link as usual.
        case foreground
        /// Backgrounded with a transfer mid-flight — draining under the grace
        /// window before the intentional disconnect.
        case draining
        /// Backgrounded, link intentionally down, transport reconnect paused.
        case suspended
    }

    public private(set) var phase: LinkPhase = .foreground

    private let transport: any DeviceTransport
    private let activity: TransferActivity
    private let backgroundTasks: any BackgroundTaskRunner

    /// The transport's last known connection — read at background time to
    /// decide whether a later foreground should re-raise the link at all.
    /// `.connecting`/`.outOfRange` count as "had a link" (the transport was
    /// trying; a foreground return should keep trying).
    @ObservationIgnored private(set) var connection: ConnectionState = .disconnected
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?
    /// The in-flight suspend — the drain-then-disconnect task, or (after an
    /// expiry) the forced disconnect. A foreground return awaits it before
    /// `resumeLink()`, so a resume can never overtake its own suspend on the
    /// transport's queue.
    @ObservationIgnored private var suspend: Task<Void, Never>?
    /// The in-flight foreground resume. The next suspend awaits it before
    /// `suspendLink()`, so a quick background→foreground→background flap can't
    /// let the resume land *after* the suspend on the transport's queue and
    /// re-raise the link while backgrounded.
    @ObservationIgnored private var resume: Task<Void, Never>?
    @ObservationIgnored private var graceToken: BackgroundGraceToken?
    /// Whether the suspended link should come back up on foreground.
    @ObservationIgnored private var resumeOnForeground = false
    @ObservationIgnored private var started = false

    public init(
        transport: any DeviceTransport,
        activity: TransferActivity,
        backgroundTasks: any BackgroundTaskRunner
    ) {
        self.transport = transport
        self.activity = activity
        self.backgroundTasks = backgroundTasks
    }

    /// Arm the connection watch (call once, from the host's `.task`).
    public func start() {
        guard !started else { return }
        started = true
        connectionWatch = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
            }
        }
    }

    deinit {
        connectionWatch?.cancel()
        suspend?.cancel()
    }

    /// The host view's one call, from `.onChange(of: scenePhase)`.
    public func scenePhaseChanged(to scenePhase: ScenePhase) {
        switch scenePhase {
        case .background:
            enterBackground()
        case .active:
            enterForeground()
        case .inactive:
            break  // shade / app-switcher flicker — never churn the link
        @unknown default:
            break
        }
    }

    // MARK: Background — drain, then suspend

    private func enterBackground() {
        guard phase == .foreground else { return }
        resumeOnForeground = connection != .disconnected
        phase = .draining
        // The grace window covers the whole suspend, not just a busy drain:
        // even the idle disconnect must finish before iOS freezes the process.
        graceToken = backgroundTasks.begin(name: "obc.link.suspend") { [weak self] in
            self?.graceExpired()
        }
        suspend = Task { [weak self] in
            guard let self else { return }
            // A resume from a just-finished foreground stint must fully land
            // before this suspend, or the two race on the transport's queue.
            await resume?.value
            await activity.waitUntilIdle()
            // Canceled = the app came back (or the grace expired and the forced
            // path owns the disconnect now) — this task must not touch anything.
            guard !Task.isCancelled, phase == .draining else { return }
            // Commit before the await: a foreground return from here on takes
            // the `.suspended` path, which awaits this task before resuming —
            // the resume can't overtake the suspend.
            phase = .suspended
            await transport.suspendLink()
            endGrace()
        }
    }

    /// The system is closing the window: disconnect NOW. The in-flight
    /// transfer stalls resumable (uploads restart, not resume — the upload
    /// sheet and the H10 sync banner already own that story); lingering past
    /// the expiry gets the app killed instead.
    private func graceExpired() {
        guard phase == .draining else {
            endGrace()
            return
        }
        suspend?.cancel()
        endGrace()  // end synchronously in the expiry handler, as UIKit expects
        phase = .suspended
        suspend = Task { [transport] in
            await transport.suspendLink()
        }
    }

    // MARK: Foreground — cancel a pending suspend, or resume a done one

    private func enterForeground() {
        switch phase {
        case .foreground:
            break
        case .draining:
            // Came right back mid-drain — the link never dropped; keep it.
            suspend?.cancel()
            suspend = nil
            endGrace()
            phase = .foreground
        case .suspended:
            phase = .foreground
            let pendingSuspend = suspend
            suspend = nil
            if resumeOnForeground {
                // The existing bonded silent-reconnect path — after any still
                // in-flight suspend has fully landed on the transport.
                resume = Task { [transport] in
                    await pendingSuspend?.value
                    await transport.resumeLink()
                }
            }
        }
        resumeOnForeground = false
    }

    private func endGrace() {
        guard let token = graceToken else { return }
        graceToken = nil
        backgroundTasks.end(token)
    }
}
