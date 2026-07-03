import Foundation

/// Whether the phone currently has a usable network path — the one signal the
/// MapKit basemap preview needs. MapKit tiles come off Apple's servers, so
/// offline (or a captive/failed path) means no basemap; the preview then
/// degrades to the grid renderer. This is **infrastructure, not UI** — it lives
/// beside `DeviceTransport` and is injected the same way (a protocol seam, so the
/// online/offline decision is unit-testable without a real radio).
///
/// The map path is the only consumer, so the surface is deliberately tiny: a
/// stream that replays the current value on subscribe, then yields on change.
public protocol NetworkReachability: Sendable {
    /// Online/offline updates. **Replays** the current value immediately on
    /// subscribe (like `DeviceTransport.state`), then emits on every change.
    var updates: AsyncStream<Bool> { get }
}

/// A fixed reachability — the whole point is testability and the forced-offline
/// launch override (`-OBCNetwork offline`). Emits `isOnline` once and holds it.
public struct ConstantReachability: NetworkReachability {
    private let isOnline: Bool

    public init(_ isOnline: Bool) {
        self.isOnline = isOnline
    }

    public var updates: AsyncStream<Bool> {
        let isOnline = self.isOnline
        return AsyncStream { continuation in
            continuation.yield(isOnline)
            // Never finishes: a finished stream would read as "no more updates",
            // and a consumer that treats stream-end as offline must not flip.
        }
    }
}

#if canImport(Network)
import Network

/// The real reachability, backed by `NWPathMonitor`. Each `updates` subscription
/// spins up its own monitor on a private queue and tears it down when the stream
/// is cancelled (the composition root holds exactly one subscription for the app
/// lifetime). `@unchecked Sendable` is safe: the only mutable state is confined
/// to the monitor's queue.
public final class PathMonitorReachability: NetworkReachability, @unchecked Sendable {
    public init() {}

    public var updates: AsyncStream<Bool> {
        AsyncStream { continuation in
            let monitor = NWPathMonitor()
            let queue = DispatchQueue(label: "com.openbikecomputer.reachability")
            monitor.pathUpdateHandler = { path in
                continuation.yield(path.status == .satisfied)
            }
            continuation.onTermination = { _ in monitor.cancel() }
            monitor.start(queue: queue)
        }
    }
}
#endif
