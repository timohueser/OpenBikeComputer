import SwiftUI
import OBCTransport

/// Whether a track preview draws the **MapKit basemap** or the **grid fallback**
/// (#294). Extracted from the view so the decision is unit-testable (the issue's
/// rule: the online/offline choice must not live inside a `View`).
public enum MapPreviewMode: Equatable, Sendable {
    /// Real Apple Maps basemap under the track polyline.
    case map
    /// The basemap-free grid + parchment placeholder (`TrackPreviewView`).
    case grid

    /// The map only shows when there's a network path **and** real geometry to
    /// draw over it; otherwise the grid is the intended, graceful fallback.
    public static func resolve(isOnline: Bool, hasCoordinates: Bool) -> MapPreviewMode {
        isOnline && hasCoordinates ? .map : .grid
    }
}

/// Observable wrapper the composition root owns: subscribes to a
/// `NetworkReachability` seam and republishes `isOnline` for SwiftUI. Injected
/// into the view tree as `\.obcIsOnline` so every `MapTrackPreviewView` reads one
/// shared signal without each parent threading it.
@MainActor
@Observable
public final class ReachabilityStore {
    public private(set) var isOnline: Bool

    private let reachability: any NetworkReachability
    @ObservationIgnored private var watch: Task<Void, Never>?

    /// `initiallyOnline` is the value shown until the first path update lands —
    /// optimistic by default so the map doesn't flash the grid on a cold launch.
    public init(_ reachability: any NetworkReachability, initiallyOnline: Bool = true) {
        self.reachability = reachability
        self.isOnline = initiallyOnline
    }

    /// Start watching (idempotent — call from `.task`).
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
    // Optimistic default: with no monitor injected (previews, the gallery, host
    // tests) a preview that has coordinates still shows its basemap.
    static let defaultValue = true
}

extension EnvironmentValues {
    /// The shared online/offline signal for the map previews (#294).
    public var obcIsOnline: Bool {
        get { self[IsOnlineKey.self] }
        set { self[IsOnlineKey.self] = newValue }
    }
}
