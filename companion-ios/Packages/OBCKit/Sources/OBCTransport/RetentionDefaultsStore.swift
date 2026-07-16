import Foundation
import OBCDomain

/// Persistence seam for the **default route retention** (epic #638 S7) — the
/// app-local preference that seeds a *new* upload's Auto-delete level. Purely a
/// phone-side setting (no device round-trip), so it lives beside ``BondStore``,
/// not on the wire: Settings reads/writes it, and an upload seeds its picker and
/// its post-commit `setRouteRetention` from it. Changing it never rewrites an
/// existing route's retention (no retro writes) — it only changes what the *next*
/// upload starts at.
public protocol RetentionDefaultsStore: Sendable {
    /// The level a fresh upload seeds — ``Retention/appDefault`` (two weeks) until
    /// the rider changes it in Settings.
    func loadDefaultRetention() -> Retention
    /// Persist the rider's chosen default. Takes effect for the next upload only.
    func saveDefaultRetention(_ retention: Retention)
}

/// The real store: one key in `UserDefaults`, mirroring ``UserDefaultsBondStore``.
/// An absent key reads as ``Retention/appDefault`` — a first launch behaves as if
/// the rider had picked the documented default. `@unchecked`: `UserDefaults` is
/// documented thread-safe but the iOS SDK doesn't annotate it `Sendable`.
public struct UserDefaultsRetentionDefaultsStore: RetentionDefaultsStore, @unchecked Sendable {
    private static let key = "obc.defaultRouteRetention"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func loadDefaultRetention() -> Retention {
        guard defaults.object(forKey: Self.key) != nil else { return .appDefault }
        return Retention(safeRawValue: UInt8(truncatingIfNeeded: defaults.integer(forKey: Self.key)))
    }

    public func saveDefaultRetention(_ retention: Retention) {
        defaults.set(Int(retention.rawValue), forKey: Self.key)
    }
}

/// An in-memory store — the default for mock/preview/test composition, so every
/// scenario-driven launch starts from ``Retention/appDefault`` (a `UserDefaults`
/// key would leak the previous run's choice into the next, exactly the
/// non-determinism the mock library-store avoids by staying in memory). A
/// reference type so the same instance, shared across the main + settings models,
/// sees the rider's change within the session.
public final class InMemoryRetentionDefaultsStore: RetentionDefaultsStore, @unchecked Sendable {
    private let lock = NSLock()
    private var value: Retention

    public init(_ initial: Retention = .appDefault) {
        self.value = initial
    }

    public func loadDefaultRetention() -> Retention {
        lock.withLock { value }
    }

    public func saveDefaultRetention(_ retention: Retention) {
        lock.withLock { value = retention }
    }
}
