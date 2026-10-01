import SwiftUI
import UIKit
import OBCUI

/// UIKit stays in the composition root. Download activity does not claim a BLE transfer token.
struct IdleTimerGuard: ViewModifier {
    let activity: TransferActivity
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.obcOfflineMaps) private var offlineMaps
    private var transferring: Bool { activity.isActive || offlineMaps?.isDownloading == true }

    func body(content: Content) -> some View {
        content
            // `initial: true` runs once at attach too: `onChange` alone never fires for a state
            // that is already true when the modifier appears, and a scene re-attach must overwrite
            // whatever stale value the UIKit flag was left holding.
            .onChange(of: transferring, initial: true) { _, _ in apply() }
            .onChange(of: scenePhase, initial: true) { _, _ in apply() }
    }

    private func apply() {
        UIApplication.shared.isIdleTimerDisabled = transferring && scenePhase == .active
    }
}

extension View {
    /// Disable the idle timer while the ledger holds an in-flight claim.
    func keepAwakeDuringTransfers(_ activity: TransferActivity) -> some View {
        modifier(IdleTimerGuard(activity: activity))
    }
}
