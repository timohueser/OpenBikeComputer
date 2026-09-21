import SwiftUI
import UIKit
import OBCUI

/// Keeps the screen awake while a transfer is in flight. `UIApplication.isIdleTimerDisabled` is
/// UIKit, so the touch lives at the composition root and the OBCKit view models never see it.
///
/// It reads the app-level in-flight ledger every transfer already claims a token from, so one
/// modifier covers route uploads, ride syncs and firmware sends uniformly, and the flag clears the
/// moment the last claim ends.
///
/// Only meaningful foregrounded: the assertion is re-derived on every scene-phase change and forced
/// off unless the app is active. iOS ignores the flag while the app is not frontmost anyway, and
/// the Bluetooth background mode, not this flag, is what keeps a backgrounded transfer alive.
struct IdleTimerGuard: ViewModifier {
    let activity: TransferActivity
    @Environment(\.scenePhase) private var scenePhase

    func body(content: Content) -> some View {
        content
            // `initial: true` runs once at attach too: `onChange` alone never fires for a state
            // that is already true when the modifier appears, and a scene re-attach must overwrite
            // whatever stale value the UIKit flag was left holding.
            .onChange(of: activity.isActive, initial: true) { _, _ in apply() }
            .onChange(of: scenePhase, initial: true) { _, _ in apply() }
    }

    private func apply() {
        UIApplication.shared.isIdleTimerDisabled = activity.isActive && scenePhase == .active
    }
}

extension View {
    /// Disable the idle timer while the ledger holds an in-flight claim.
    func keepAwakeDuringTransfers(_ activity: TransferActivity) -> some View {
        modifier(IdleTimerGuard(activity: activity))
    }
}
