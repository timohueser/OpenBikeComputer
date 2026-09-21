import BackgroundTasks
import Foundation
import OBCTransport

/// The background half of the proactive update surfaces: while the app is away, ask once in a
/// while whether a firmware update has been published, and if one has, for the device this phone
/// last talked to, leave a local notification about it.
///
/// Best-effort by design, and that is the system's call. A background refresh task is a request:
/// iOS decides whether and when to run it, weighing battery, network, and how much the rider uses
/// the app. A phone that never wakes us simply never notifies, and the launch sheet covers that
/// case the next time the app opens. Nothing in the update flow depends on this firing.
///
/// The whole body is a handful of lines, because the decision is not here: it is
/// ``UpdateSurfaceRunner``, shared verbatim with the launch sheet, so a background wake can never
/// notify about something the sheet would not have raised. The registration itself is the scene's
/// `.backgroundTask(.appRefresh:)` modifier.
enum BackgroundUpdateRefresh {
    /// The task identifier. It must match the permitted identifiers in `project.yml`: iOS refuses,
    /// loudly and at launch, to register an identifier the Info.plist does not list.
    static let identifier = "com.openbikecomputer.companion.updatecheck"

    /// How far out to ask for the next wake. Comfortably past the check's cache window, so a wake
    /// that does happen has a real question to ask instead of reading a fresh cache and going back
    /// to sleep, which is also how iOS learns the task is worth running.
    static let interval: TimeInterval = 8 * 60 * 60

    /// Ask for the next wake. Submitted when the app goes to the background and again at the end
    /// of every run: a refresh request is one-shot, so a run that does not re-submit is the last
    /// one.
    ///
    /// Silent on failure by intent: the throw cases are "not permitted", such as a simulator or a
    /// device with Background App Refresh switched off, and "too many pending". Neither is
    /// something the rider can act on, and the proactive surface degrades to the launch sheet.
    /// Turning automatic checks off needs no cancellation pass: this guard stops the next request,
    /// and an already-pending wake finds the toggle off, decides nothing, and does not re-submit.
    static func schedule(from now: Date = Date()) {
        guard UpdateSurfaceRunner().autoCheckEnabled else { return }
        let request = BGAppRefreshTaskRequest(identifier: identifier)
        request.earliestBeginDate = now.addingTimeInterval(interval)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// The wake itself: decide, which may fetch, notify if there is something unanswered, and ask
    /// for the next wake either way.
    ///
    /// Note what is not here: no BLE, no device read. The link is down by definition, which is why
    /// the running version was persisted while it could be read. A phone that has never seen a
    /// device has nothing to compare and says nothing.
    static func run(
        runner: UpdateSurfaceRunner = UpdateSurfaceRunner(),
        notifier: any UpdateNotifying = SystemUpdateNotifier(),
        bondStore: any BondStore = UserDefaultsBondStore()
    ) async {
        defer { schedule() }
        // Keep the decision and its ledger write attached to one snapshot. This is normally a
        // no-link wake, but it also makes the function correct if the app or device state changes
        // while its network request is in flight.
        let device = runner.device()
        guard let release = await runner.run(device: device) else { return }
        let posted = await notifier.notifyUpdateAvailable(
            version: release.version,
            deviceName: bondStore.load()?.deviceName ?? "your bike computer"
        )
        // Only a notice that actually reached the rider counts as an answer. Tapping it opens the
        // update screen, and ignoring it is still an answer, exactly as dismissing the launch sheet
        // is: one notice per version, never a nag. A denied notifier records nothing, so the offer
        // survives for the launch sheet.
        guard posted else { return }
        runner.recordAnswered(version: release.version, device: device)
    }
}
