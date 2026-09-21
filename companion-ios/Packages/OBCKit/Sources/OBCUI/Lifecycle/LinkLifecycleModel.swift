import SwiftUI
import Observation
import OBCDomain
import OBCTransport

/// The foreground-only link policy: the app holds the BLE link only while foregrounded, because
/// holding it in the background burns battery on both ends for no user-visible benefit.
///
/// The rules:
/// - A real `.background` transition suspends the link. An `.inactive` flicker, such as the
///   notification shade or the app switcher, never churns it. No timers.
/// - An in-flight transfer or sync is never dropped: while the `TransferActivity` ledger is
///   non-empty, the suspend drains under a system grace window and disconnects after. An idle
///   link suspends promptly.
/// - The suspend goes through `DeviceLink.suspendLink()`, whose contract is to drop the link and
///   pause the transport's reconnect loop, or the loop fights the intentional disconnect.
/// - Foreground re-raises through `resumeLink()`, the existing bonded silent-reconnect path, but
///   only when a link existed at suspend time: a never-paired session must not start scanning
///   because the rider checked a text. The reconnect's edge into `.connected` is what triggers
///   the main screen's reload, truing up anything that changed while the app was away.
///
/// Depends only on `DeviceLink` and the two seams. The host view feeds it raw scene-phase changes;
/// UIKit stays in the app target.
@MainActor @Observable
public final class LinkLifecycleModel {
    /// Where the policy currently stands. Exposed for the host and the tests; nothing user-facing
    /// renders it.
    public enum LinkPhase: Equatable, Sendable {
        /// Foregrounded: the transport owns the link as usual.
        case foreground
        /// Backgrounded with a transfer mid-flight, draining under the grace window before the
        /// intentional disconnect.
        case draining
        /// Backgrounded, link intentionally down, transport reconnect paused.
        case suspended
    }

    public private(set) var phase: LinkPhase = .foreground

    private let transport: any DeviceLink
    private let activity: TransferActivity
    private let backgroundTasks: any BackgroundTaskRunner

    /// The transport's last known connection, read at background time to decide whether a later
    /// foreground should re-raise the link at all. `.connecting` and `.outOfRange` count as "had a
    /// link", because the transport was trying and a foreground return should keep trying.
    @ObservationIgnored private(set) var connection: ConnectionState = .disconnected
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?
    /// The in-flight suspend: the drain-then-disconnect task, or, after an expiry, the forced
    /// disconnect. A foreground return awaits it before `resumeLink()`, so a resume can never
    /// overtake its own suspend on the transport's queue.
    @ObservationIgnored private var suspend: Task<Void, Never>?
    /// The in-flight foreground resume. The next suspend awaits it before `suspendLink()`, so a
    /// quick background, foreground, background flap cannot let the resume land after the suspend
    /// on the transport's queue and re-raise the link while backgrounded.
    @ObservationIgnored private var resume: Task<Void, Never>?
    @ObservationIgnored private var graceToken: BackgroundGraceToken?
    /// Whether the suspended link should come back up on foreground.
    @ObservationIgnored private var resumeOnForeground = false
    @ObservationIgnored private var started = false

    public init(
        transport: any DeviceLink,
        activity: TransferActivity,
        backgroundTasks: any BackgroundTaskRunner
    ) {
        self.transport = transport
        self.activity = activity
        self.backgroundTasks = backgroundTasks
    }

    /// Arm the connection watch. Call once.
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
            break  // a shade or app-switcher flicker never churns the link
        @unknown default:
            break
        }
    }

    // MARK: Background: drain, then suspend

    private func enterBackground() {
        guard phase == .foreground else { return }
        resumeOnForeground = connection != .disconnected
        phase = .draining
        // The grace window covers the whole suspend, not just a busy drain: even the idle
        // disconnect must finish before iOS freezes the process.
        graceToken = backgroundTasks.begin(name: "obc.link.suspend") { [weak self] in
            self?.graceExpired()
        }
        suspend = Task { [weak self] in
            guard let self else { return }
            // A resume from a just-finished foreground stint must fully land before this suspend,
            // or the two race on the transport's queue.
            await resume?.value
            await activity.waitUntilIdle()
            // Cancelled means the app came back, or the grace expired and the forced path owns
            // the disconnect now, so this task must not touch anything.
            guard !Task.isCancelled, phase == .draining else { return }
            // Commit before the await: a foreground return from here on takes the suspended path,
            // which awaits this task before resuming, so the resume cannot overtake the suspend.
            phase = .suspended
            await transport.suspendLink()
            endGrace()
        }
    }

    /// The system is closing the window, so disconnect now. The in-flight transfer stalls
    /// resumable, and the upload sheet and the sync banner already own that story; lingering past
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

    // MARK: Foreground: cancel a pending suspend, or resume a done one

    private func enterForeground() {
        switch phase {
        case .foreground:
            break
        case .draining:
            // Came right back mid-drain: the link never dropped, so keep it.
            suspend?.cancel()
            suspend = nil
            endGrace()
            phase = .foreground
        case .suspended:
            phase = .foreground
            let pendingSuspend = suspend
            suspend = nil
            if resumeOnForeground {
                // The existing bonded silent-reconnect path, after any still in-flight suspend
                // has fully landed on the transport.
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
