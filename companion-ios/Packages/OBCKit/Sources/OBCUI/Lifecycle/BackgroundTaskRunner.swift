import Foundation

/// A claimed system grace window, opaque beyond the raw id the platform handed out.
public struct BackgroundGraceToken: Equatable, Sendable {
    public let rawValue: Int

    public init(rawValue: Int) {
        self.rawValue = rawValue
    }
}

/// The `beginBackgroundTask` seam. `LinkLifecycleModel` needs a system grace window
/// to drain an in-flight transfer past a background transition, but that API is UIKit
/// and the lifecycle logic lives here, where it runs under `swift test`. The app
/// target supplies the real implementation; tests supply a spy.
public protocol BackgroundTaskRunner: Sendable {
    /// Ask the system for a grace window. `onExpiry` fires on the main actor when the
    /// system is about to close it: the caller must wind down and `end` the token, or
    /// iOS kills the app. Returns `nil` when the platform refused one.
    @MainActor func begin(
        name: String,
        onExpiry: @escaping @MainActor @Sendable () -> Void
    ) -> BackgroundGraceToken?

    /// Give the window back, exactly once per token. The model guards this, and an
    /// implementation may treat a stray double-end as a programmer error.
    @MainActor func end(_ token: BackgroundGraceToken)
}
