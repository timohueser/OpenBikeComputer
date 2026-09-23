import MapKit
import OBCDomain

/// Stops from Apple Maps: campsites and hotels around a point, and a free place search.
struct AppleMapsStopSearch: StopSearch {
    /// `MKLocalPointsOfInterestRequest` is not used: it clamps its radius to 2 km and returns
    /// nothing farther than about 1 km, so a day end at the edge of a town misses the town.
    func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        async let hotels = Self.stops(matching: "hotel", [.hotel], near: center, radius: radius)
        async let campsites = Self.stops(matching: "camping", Self.campsites, near: center, radius: radius)
        return try await campsites + hotels
    }

    /// Apple files some campsites as RV parks.
    private static var campsites: [MKPointOfInterestCategory] {
        if #available(iOS 18, *) { [.campground, .rvPark] } else { [.campground] }
    }

    private static func stops(
        matching query: String, _ categories: [MKPointOfInterestCategory], near center: Coordinate, radius: Double
    ) async throws -> [Stop] {
        let request = MKLocalSearch.Request()
        request.naturalLanguageQuery = query
        request.resultTypes = .pointOfInterest
        request.pointOfInterestFilter = MKPointOfInterestFilter(including: categories)
        request.region = MKCoordinateRegion(
            center: CLLocationCoordinate2D(latitude: center.latitude, longitude: center.longitude),
            latitudinalMeters: 2 * radius, longitudinalMeters: 2 * radius)
        if #available(iOS 18, *) { request.regionPriority = .required }
        return try await run(MKLocalSearch(request: request))
    }

    func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop] {
        let request = MKLocalSearch.Request()
        request.naturalLanguageQuery = query
        request.region = MKCoordinateRegion(
            center: CLLocationCoordinate2D(
                latitude: (southWest.latitude + northEast.latitude) / 2,
                longitude: (southWest.longitude + northEast.longitude) / 2),
            span: MKCoordinateSpan(
                latitudeDelta: northEast.latitude - southWest.latitude,
                longitudeDelta: northEast.longitude - southWest.longitude))
        // Without it the region is only a hint, and "Münster" finds the city in Germany.
        if #available(iOS 18, *) { request.regionPriority = .required }
        return try await Self.run(MKLocalSearch(request: request))
    }

    /// MapKit reports "nothing here" as an error, not as an empty answer.
    private static func run(_ search: MKLocalSearch) async throws -> [Stop] {
        do {
            return try await search.start().mapItems.compactMap(stop)
        } catch MKError.placemarkNotFound {
            return []
        }
    }

    private static func stop(_ item: MKMapItem) -> Stop? {
        guard let name = item.name else { return nil }
        let kind: Stop.Kind = switch item.pointOfInterestCategory {
        case .hotel?: .hotel
        case let category? where campsites.contains(category): .campsite
        default: .place
        }
        var identifier: String?
        if #available(iOS 18, *) { identifier = item.identifier?.rawValue }
        let coordinate = item.placemark.coordinate
        return Stop(
            name: name, coordinate: Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude),
            kind: kind, mapItemID: identifier)
    }
}
