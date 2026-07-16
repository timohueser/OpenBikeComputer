import UIKit
import OBCUI

/// The real `BackgroundTaskRunner` (#459) — the one place `beginBackgroundTask`
/// is allowed, app-target on purpose: the lifecycle logic lives in OBCKit where
/// it runs under `swift test`, and UIKit stays at the composition root (the
/// same rule that keeps CoreBluetooth inside `OBCTransport/BLE/`).
struct UIKitBackgroundTaskRunner: BackgroundTaskRunner {
    @MainActor func begin(
        name: String,
        onExpiry: @escaping @MainActor @Sendable () -> Void
    ) -> BackgroundGraceToken? {
        let id = UIApplication.shared.beginBackgroundTask(withName: name) {
            // UIKit calls the expiration handler on the main thread.
            MainActor.assumeIsolated { onExpiry() }
        }
        guard id != .invalid else { return nil }
        return BackgroundGraceToken(rawValue: id.rawValue)
    }

    @MainActor func end(_ token: BackgroundGraceToken) {
        UIApplication.shared.endBackgroundTask(UIBackgroundTaskIdentifier(rawValue: token.rawValue))
    }
}
