import CoreLocation
import OBCDomain

/// One position fix, the start of a route to a shared place. The first call asks for When In Use
/// access. CoreLocation calls the delegate on the thread that made the manager, the main thread.
@MainActor
final class CurrentLocation: NSObject, CLLocationManagerDelegate {
    private let manager = CLLocationManager()
    private var continuation: CheckedContinuation<Coordinate?, Never>?

    /// Nil when the rider declines, or when no fix arrives within ten seconds.
    static func fix() async -> Coordinate? {
        await CurrentLocation().locate()
    }

    private func locate() async -> Coordinate? {
        let timeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(10))
            self?.finish(nil)
        }
        defer { timeout.cancel() }
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                self.continuation = continuation
                manager.desiredAccuracy = kCLLocationAccuracyNearestTenMeters
                manager.delegate = self
                start(manager.authorizationStatus)
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.finish(nil) }
        }
    }

    /// Runs again on every authorization change, so the fix starts once the rider answers the prompt.
    private func start(_ status: CLAuthorizationStatus) {
        switch status {
        case .notDetermined: manager.requestWhenInUseAuthorization()
        case .authorizedWhenInUse, .authorizedAlways: manager.requestLocation()
        default: finish(nil)
        }
    }

    private func finish(_ coordinate: Coordinate?) {
        continuation?.resume(returning: coordinate)
        continuation = nil
    }

    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        let status = manager.authorizationStatus
        MainActor.assumeIsolated { start(status) }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        let coordinate = locations.last(where: { $0.horizontalAccuracy >= 0 })
            .map { Coordinate(latitude: $0.coordinate.latitude, longitude: $0.coordinate.longitude) }
        MainActor.assumeIsolated { finish(coordinate) }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: any Error) {
        MainActor.assumeIsolated { finish(nil) }
    }
}
