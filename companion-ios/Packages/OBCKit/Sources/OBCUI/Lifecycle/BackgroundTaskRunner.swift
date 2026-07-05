import Foundation

/// A claimed system grace window — opaque beyond the raw id the platform
/// handed out (`UIBackgroundTaskIdentifier.rawValue` on the real path).
public struct BackgroundGraceToken: Equatable, Sendable {
    public let rawValue: Int

    public init(rawValue: Int) {
        self.rawValue = rawValue
    }
}

/// The `beginBackgroundTask` seam (#459). `LinkLifecycleModel` needs a system
/// grace window to drain an in-flight transfer past a background transition —
/// but that API is UIKit, and the lifecycle logic lives here in the package
/// where it runs under `swift test`. The app target supplies the real
/// `UIApplication` implementation; tests supply a spy (the same pattern as
/// `DeviceTransport` itself — UIKit never leaks below the composition root).
public protocol BackgroundTaskRunner: Sendable {
    /// Ask the system for a grace window. `onExpiry` fires on the main actor
    /// when the system is about to close it (the caller must wind down and
    /// `end` the token promptly, or iOS kills the app). Returns `nil` when the
    /// platform refused one.
    @MainActor func begin(
        name: String,
        onExpiry: @escaping @MainActor @Sendable () -> Void
    ) -> BackgroundGraceToken?

    /// Give the window back. Exactly once per token — the model guards this;
    /// implementations may treat a stray double-end as a programmer error.
    @MainActor func end(_ token: BackgroundGraceToken)
}
