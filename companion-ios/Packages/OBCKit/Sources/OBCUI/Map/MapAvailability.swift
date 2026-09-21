import SwiftUI
import OBCTransport

/// Whether a track preview draws the MapKit basemap or the grid fallback. Extracted
/// from the view so the decision is unit-testable.
public enum MapPreviewMode: Equatable, Sendable {
    case map
    case grid

    /// The map only shows with a network path and real geometry; otherwise the grid
    /// is the intended fallback.
    public static func resolve(isOnline: Bool, hasCoordinates: Bool) -> MapPreviewMode {
        isOnline && hasCoordinates ? .map : .grid
    }
}

/// Observable wrapper the composition root owns: it subscribes to a
/// `NetworkReachability` seam and republishes `isOnline` for SwiftUI. Injected as
/// `\.obcIsOnline` so every preview reads one shared signal.
@MainActor
@Observable
public final class ReachabilityStore {
    public private(set) var isOnline: Bool

    private let reachability: any NetworkReachability
    @ObservationIgnored private var watch: Task<Void, Never>?

    /// `initiallyOnline` is the value shown until the first path update lands. It is
    /// optimistic by default so the map does not flash the grid on a cold launch.
    public init(_ reachability: any NetworkReachability, initiallyOnline: Bool = true) {
        self.reachability = reachability
        self.isOnline = initiallyOnline
    }

    /// Start watching. Idempotent; call it from `.task`.
    public func start() {
        guard watch == nil else { return }
        watch = Task { [weak self, reachability] in
            for await online in reachability.updates {
                self?.isOnline = online
            }
        }
    }

    deinit { watch?.cancel() }
}

private struct IsOnlineKey: EnvironmentKey {
    // Optimistic default: with no monitor injected (previews, the gallery, host tests)
    // a preview that has coordinates still shows its basemap.
    static let defaultValue = true
}

extension EnvironmentValues {
    /// The shared online signal for the map previews.
    public var obcIsOnline: Bool {
        get { self[IsOnlineKey.self] }
        set { self[IsOnlineKey.self] = newValue }
    }
}
