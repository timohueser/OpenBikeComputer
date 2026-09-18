import CoreLocation
import CoreMotion
import OBCHost
import UIKit

/// The phone's GPS, compass, barometer and battery on the host's sensor ports.
///
/// CoreLocation delivers on the thread that made the manager — the main thread — and every push is
/// main thread only, so each `nonisolated` delegate method states the isolation it already has
/// rather than hopping through a task.
@MainActor
final class PhoneSensors: NSObject, CLLocationManagerDelegate {
    private var host: OpaquePointer?
    private let locations = CLLocationManager()
    private let altimeter = CMAltimeter()
    /// The barometer has answered at least once, which makes a fix's altitude the weaker second
    /// source. Set by the first sample, not by `isAbsoluteAltitudeAvailable`: the simulator claims
    /// a barometer and then says nothing, and no altitude at all is worse than a GPS one.
    private var hasBarometer = false

    override init() {
        super.init()
        locations.delegate = self
    }

    /// Start every sensor on an open host.
    func attach(to host: OpaquePointer) {
        self.host = host
        locations.requestWhenInUseAuthorization()
        locations.desiredAccuracy = kCLLocationAccuracyBestForNavigation
        locations.distanceFilter = kCLDistanceFilterNone
        locations.activityType = .otherNavigation
        locations.pausesLocationUpdatesAutomatically = false
        locations.startUpdatingLocation()
        locations.headingFilter = 1
        locations.headingOrientation = .portrait
        locations.startUpdatingHeading()

        if CMAltimeter.isAbsoluteAltitudeAvailable() {
            altimeter.startAbsoluteAltitudeUpdates(to: .main) { [weak self] altitude, _ in
                guard let metres = altitude?.altitude else { return }
                MainActor.assumeIsolated {
                    self?.hasBarometer = true
                    self?.push(altitude: Float(metres))
                }
            }
        }

        UIDevice.current.isBatteryMonitoringEnabled = true
        pushBattery()
        NotificationCenter.default.addObserver(
            self, selector: #selector(pushBattery),
            name: UIDevice.batteryLevelDidChangeNotification, object: nil)

        #if DEBUG
            restartPretendTimer()
        #endif
    }

    /// Stop every sensor. The host pointer goes first: nothing must reach a closed host.
    func detach() {
        host = nil
        hasBarometer = false
        locations.stopUpdatingLocation()
        locations.stopUpdatingHeading()
        altimeter.stopAbsoluteAltitudeUpdates()
        UIDevice.current.isBatteryMonitoringEnabled = false
        NotificationCenter.default.removeObserver(
            self, name: UIDevice.batteryLevelDidChangeNotification, object: nil)
        #if DEBUG
            // The place itself stays set: `attach(to:)` starts the timer again on a reopen.
            pretendTimer?.invalidate()
            pretendTimer = nil
        #endif
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        MainActor.assumeIsolated {
            #if DEBUG
                if pretend != nil { return }
            #endif
            for location in locations where location.horizontalAccuracy >= 0 { push(location) }
        }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateHeading newHeading: CLHeading) {
        // A phone without a calibrated declination reports −1 true heading and a usable magnetic
        // one. Read both here: `CLHeading` is a class and may not cross into the actor.
        let degrees = newHeading.trueHeading >= 0 ? newHeading.trueHeading : newHeading.magneticHeading
        MainActor.assumeIsolated {
            guard let host else { return }
            obc_ios_push_heading(host, Float(degrees))
        }
    }

    private func push(_ location: CLLocation) {
        guard let host else { return }
        obc_ios_push_fix(
            host,
            Int32((location.coordinate.latitude * 1e6).rounded()),
            Int32((location.coordinate.longitude * 1e6).rounded()),
            // A negative course or speed is CoreLocation saying it does not know.
            location.course >= 0 ? Float(location.course) : .nan,
            location.speed >= 0 ? Float(location.speed) : .nan,
            // Clamped, not converted: a stamp outside the range is a fix the host reads as
            // unstamped, never a trap.
            UInt32(clamping: Int64(location.timestamp.timeIntervalSince1970)))
        if !hasBarometer, location.verticalAccuracy >= 0 {
            push(altitude: Float(location.altitude))
        }
    }

    #if DEBUG
        /// A pretend position, set from the developer sheet. While it holds a coordinate the real
        /// fixes are dropped and this one goes to the host once a second, the cadence a real fix
        /// has: the app then sees a live rider who stands still, not one fix that goes stale.
        ///
        /// The compass, the barometer and the battery stay real, and nothing here is saved.
        var pretend: CLLocationCoordinate2D? {
            didSet { restartPretendTimer() }
        }

        private var pretendTimer: Timer?

        /// Push the place now and once a second from here. Common mode, as the display link uses:
        /// a `.default` timer stops while a list scrolls, and five seconds of that is the app's
        /// "No GPS Fix".
        private func restartPretendTimer() {
            pretendTimer?.invalidate()
            pretendTimer = nil
            guard let pretend else { return }
            push(pretend)
            let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.push(pretend) }
            }
            RunLoop.main.add(timer, forMode: .common)
            pretendTimer = timer
        }

        /// A rider who stands still: no course, a speed of zero, and the stamp of this second, so
        /// the wall clock stays right and the fix is never stale.
        private func push(_ coordinate: CLLocationCoordinate2D) {
            guard let host else { return }
            obc_ios_push_fix(
                host,
                Int32((coordinate.latitude * 1e6).rounded()),
                Int32((coordinate.longitude * 1e6).rounded()),
                .nan, 0,
                UInt32(clamping: Int64(Date.now.timeIntervalSince1970)))
        }
    #endif

    private func push(altitude metres: Float) {
        guard let host else { return }
        obc_ios_push_altitude(host, metres)
    }

    /// The simulator reports −1 and gets no battery at all, which is better than a wrong one.
    @objc private func pushBattery() {
        let level = UIDevice.current.batteryLevel
        guard let host, level >= 0 else { return }
        obc_ios_push_battery(host, UInt8((level * 100).rounded()))
    }
}
