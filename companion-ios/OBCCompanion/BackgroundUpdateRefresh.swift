import BackgroundTasks
import Foundation
import OBCTransport

/// The background half of #773 U5: while the app is away, ask once in a while whether a firmware
/// update has been published, and if one has — for the device this phone last talked to — leave a
/// local notification about it.
///
/// **Best-effort by design, and that is iOS's call, not ours.** A `BGAppRefreshTask` is a *request*:
/// the system decides whether and when to run it, weighing battery, network, and how much the rider
/// actually uses the app. A phone that never wakes us simply never notifies, and the launch sheet
/// covers that case the next time the app opens. Nothing in the update flow depends on this firing —
/// it is a courtesy, not a channel.
///
/// The whole body is a handful of lines because the decision isn't here: it is
/// ``UpdateSurfaceRunner``, shared verbatim with the launch sheet, so a background wake can never
/// notify about something the sheet wouldn't have raised (and vice versa). The registration itself
/// is SwiftUI's `.backgroundTask(.appRefresh:)` on the scene — see `OBCCompanionApp`.
enum BackgroundUpdateRefresh {
    /// The task identifier. Must match `BGTaskSchedulerPermittedIdentifiers` in `project.yml` —
    /// iOS refuses to register an identifier the Info.plist doesn't list, loudly, at launch.
    static let identifier = "com.openbikecomputer.companion.updatecheck"

    /// How far out to ask for the next wake. Comfortably past U4's 6-hour cache window, so a wake
    /// that does happen has a real question to ask instead of reading a fresh cache and going back
    /// to sleep (which is also how iOS learns the task is worth running).
    static let interval: TimeInterval = 8 * 60 * 60

    /// Ask for the next wake. Submitted when the app goes to the background and again at the end of
    /// every run — a `BGAppRefreshTaskRequest` is one-shot, so a run that doesn't re-submit is the
    /// last one.
    ///
    /// Silent on failure by intent: the throw cases are "not permitted" (a simulator, a device with
    /// Background App Refresh switched off) and "too many pending", neither of which the rider can
    /// or should do anything about. The proactive surface degrades to the launch sheet.
    /// Turning automatic checks off needs no cancellation pass: this guard stops the *next* request
    /// from being submitted, and an already-pending wake finds the toggle off, decides nothing, and
    /// doesn't re-submit. The switch is self-healing in at most one wake.
    static func schedule(from now: Date = Date()) {
        guard UpdateSurfaceRunner().autoCheckEnabled else { return }
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = now.addingTimeInterval(interval)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// The wake itself: decide (which may fetch), notify if there's something unanswered, and ask
    /// for the next wake either way.
    ///
    /// Note what is **not** here: no BLE, no device read. The link is down by definition — that is
    /// why the running version was persisted while it could be read (`LastSeenDevice`). A phone that
    /// has never seen a device has nothing to compare and says nothing.
    static func run(
        runner: UpdateSurfaceRunner = UpdateSurfaceRunner(),
        notifier: any UpdateNotifying = SystemUpdateNotifier(),
        bondStore: any BondStore = UserDefaultsBondStore()
    ) async {
        defer { schedule() }
        guard let release = await runner.run() else { return }
        let posted = await notifier.notifyUpdateAvailable(
            version: release.version,
            deviceName: bondStore.load()?.deviceName ?? "your bike computer"
        )
        // Only a notice that actually reached the rider counts as an answer. Tapping it opens S7;
        // ignoring it is still an answer, exactly as dismissing the launch sheet is — one notice per
        // version, never a nag, and the sheet won't re-raise what the notification already raised.
        // A *denied* notifier records nothing, so the offer survives for the launch sheet.
        guard posted else { return }
        runner.recordAnswered(version: release.version, device: runner.device())
    }
}
