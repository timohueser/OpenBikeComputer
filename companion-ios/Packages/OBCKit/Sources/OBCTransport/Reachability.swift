import Foundation

/// Whether the phone has a usable network path: the one signal the MapKit basemap preview
/// needs. MapKit tiles come off Apple's servers, so offline means no basemap and the preview
/// degrades to the grid renderer. A protocol seam, so the decision is testable with no radio.
public protocol NetworkReachability: Sendable {
    /// Online and offline updates. It replays the current value immediately on subscribe, then
    /// emits on every change.
    var updates: AsyncStream<Bool> { get }
}

/// A fixed reachability, for tests and the `-OBCNetwork offline` launch override. It emits
/// `isOnline` once and holds it.
public struct ConstantReachability: NetworkReachability {
    private let isOnline: Bool

    public init(_ isOnline: Bool) {
        self.isOnline = isOnline
    }

    public var updates: AsyncStream<Bool> {
        let isOnline = self.isOnline
        return AsyncStream { continuation in
            continuation.yield(isOnline)
            // Never finishes: a consumer that reads the end of the stream as offline must not
            // flip.
        }
    }
}

#if canImport(Network)
import Network

/// The real reachability, backed by `NWPathMonitor`. Each `updates` subscription starts its own
/// monitor on a private queue and cancels it when the stream ends. `@unchecked Sendable` is
/// safe: the only mutable state is confined to the monitor's queue.
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
