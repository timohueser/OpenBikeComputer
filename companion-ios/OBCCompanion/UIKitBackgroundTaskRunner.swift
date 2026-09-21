import UIKit
import OBCUI

/// The real `BackgroundTaskRunner`: the one place `beginBackgroundTask` is allowed. App-target on
/// purpose, because the lifecycle logic lives in OBCKit where it runs under `swift test`, and UIKit
/// stays at the composition root.
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
