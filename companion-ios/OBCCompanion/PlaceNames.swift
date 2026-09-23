import CoreLocation
import OBCDomain

/// Names for places on a trip, from Apple's reverse geocoder.
enum PlaceNames {
    /// The locality at `coordinate`, such as "Ulrichen". Nil offline, on a failed lookup, or
    /// where the place has no locality.
    @Sendable static func locality(at coordinate: Coordinate) async -> String? {
        let location = CLLocation(latitude: coordinate.latitude, longitude: coordinate.longitude)
        let placemarks = try? await CLGeocoder().reverseGeocodeLocation(location)
        return placemarks?.first?.locality
    }
}
