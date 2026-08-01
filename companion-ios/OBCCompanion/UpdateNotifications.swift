import Foundation
import Observation
import UserNotifications
import OBCTransport

/// The notification half of #773 U5's proactive surfaces, in the app target because this is where
/// Apple's frameworks are allowed to live (the same rule that keeps CoreBluetooth in `BLETransport`
/// and UIKit in `UIKitBackgroundTaskRunner`).
///
/// Both types below are **deliberately dumb**: one turns a decision the policy already made into a
/// `UNNotificationRequest`, the other turns a tap into a route. Neither decides anything, which is
/// why the tests cover ``UpdateSurfacePolicy`` and not these — there is no branch here worth
/// pinning, and pinning `UNUserNotificationCenter` would only test Apple.

/// The real notifier.
///
/// **Provisional authorization on purpose.** A provisional ask never shows a system prompt: the
/// first notice is delivered quietly to Notification Center, and the rider decides from the notice
/// itself whether to keep them. That suits an update notice exactly — it is useful, it is rare, and
/// it must never be the reason a permission alert lands on someone mid-ride. Denial (or a rider who
/// turns them off from the notice) is silence, not an error: the launch sheet is the surface that
/// needs no permission, and it keeps working.
struct SystemUpdateNotifier: UpdateNotifying {
    /// The tap-routing key, read by ``UpdateNotificationDelegate``.
    static let routeKey = "obc.route"
    static let firmwareRoute = "firmwareUpdate"

    func requestAuthorization() async {
        _ = try? await UNUserNotificationCenter.current()
            .requestAuthorization(options: [.alert, .sound, .provisional])
    }

    func notifyUpdateAvailable(version: String, deviceName: String) async -> Bool {
        let center = UNUserNotificationCenter.current()
        // Ask rather than assume: a rider who declined gets silence, and posting into a denied
        // center would only burn a background wake.
        let settings = await center.notificationSettings()
        guard settings.authorizationStatus == .authorized
            || settings.authorizationStatus == .provisional
        else { return false }

        let content = UNMutableNotificationContent()
        content.title = UpdateNoticeCopy.title(version: version)
        content.body = UpdateNoticeCopy.body(deviceName: deviceName)
        content.userInfo = [Self.routeKey: Self.firmwareRoute]
        // One identifier per version: a second wake that finds the same update **replaces** the
        // pending notice instead of stacking a second copy of the same news.
        let request = UNNotificationRequest(
            identifier: "obc.firmwareUpdate.\(version)",
            content: content,
            // nil trigger = deliver now. The background task already waited for iOS's own timing;
            // adding a delay of our own would only make the notice arrive after the app is gone.
            trigger: nil
        )
        do {
            try await center.add(request)
            return true
        } catch {
            return false
        }
    }
}

/// Where a tapped notice wants to go. One flag, read and cleared by `RootView` — a shared
/// observable is enough, and it keeps the delegate free of any knowledge of the navigation stack.
@MainActor @Observable
final class UpdateRouteRequest {
    static let shared = UpdateRouteRequest()

    /// Set when the rider taps an update notice; `RootView` pushes S7 and clears it.
    var openFirmwareUpdate = false

    func consume() -> Bool {
        guard openFirmwareUpdate else { return false }
        openFirmwareUpdate = false
        return true
    }
}

/// Tap routing. `NSObject` because `UNUserNotificationCenterDelegate` requires it.
final class UpdateNotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let route = response.notification.request.content.userInfo[SystemUpdateNotifier.routeKey]
        guard route as? String == SystemUpdateNotifier.firmwareRoute else { return }
        await MainActor.run { UpdateRouteRequest.shared.openFirmwareUpdate = true }
    }
}
