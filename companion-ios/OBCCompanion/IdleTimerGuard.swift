import SwiftUI
import UIKit
import OBCUI

/// Keeps the screen awake while a transfer is in flight (#754). `UIApplication
/// .isIdleTimerDisabled` is UIKit, so — like `UIKitBackgroundTaskRunner` — the
/// touch lives at the composition root; the OBCKit view models never see it
/// (they depend only on capability-sized `OBCTransport` protocols + the observable
/// `TransferActivity` ledger — the golden rule).
///
/// It reads the app-level in-flight ledger, which every transfer already claims
/// a token from — route uploads (`UploadSheetModel`), ride syncs
/// (`RideSyncCoordinator`), and firmware sends (`FirmwareUpdateModel`) — so one
/// modifier covers them all uniformly, and the flag clears the moment the last
/// claim ends.
///
/// Only meaningful foregrounded: the assertion is re-derived on every
/// scene-phase change and forced off unless the app is `.active`. iOS ignores
/// `isIdleTimerDisabled` while the app isn't frontmost anyway — the
/// `bluetooth-central` background mode (project.yml), not this flag, is what
/// keeps a backgrounded transfer alive.
struct IdleTimerGuard: ViewModifier {
    let activity: TransferActivity
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            // `initial: true` (iOS 17): run once at attach too — `onChange`
            // alone never fires for a state that's already true when the
            // modifier appears, and a scene re-attach must overwrite whatever
            // stale value the UIKit flag was left holding.
            .onChange(of: activity.isActive, initial: true) { _, _ in apply() }
            .onChange(of: scenePhase, initial: true) { _, _ in apply() }
    }

    private func apply() {
        UIApplication.shared.isIdleTimerDisabled = activity.isActive && scenePhase == .active
    }
}

extension View {
    /// Disable the idle timer while the #754 ledger holds an in-flight claim.
    func keepAwakeDuringTransfers(_ activity: TransferActivity) -> some View {
        modifier(IdleTimerGuard(activity: activity))
    }
}
