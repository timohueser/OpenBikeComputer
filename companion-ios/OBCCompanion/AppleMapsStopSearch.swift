import MapKit
import OBCDomain

/// Stops from Apple Maps: campsites and hotels around a point, and a free place search.
struct AppleMapsStopSearch: StopSearch {
    func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        let request = MKLocalPointsOfInterestRequest(
            center: CLLocationCoordinate2D(latitude: center.latitude, longitude: center.longitude), radius: radius)
        request.pointOfInterestFilter = MKPointOfInterestFilter(including: [.campground, .hotel])
        return try await Self.run(MKLocalSearch(request: request))
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
        case .campground?: .campsite
        case .hotel?: .hotel
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
