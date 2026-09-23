#if DEBUG
import SwiftUI
import OBCDomain

/// The stops sheet on the sample Alps line, with the day end at Ulrichen: online, offline, and
/// online with nothing near.
struct TripStopsGallerySection: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Online")
            sheet(GalleryStops.nearby, isOnline: true).frame(height: 560)
            OBCEyebrow("Offline")
            sheet(GalleryStops.nearby, isOnline: false).frame(height: 250)
            OBCEyebrow("Empty")
            sheet([], isOnline: true, waypoints: false).frame(height: 200)
        }
    }

    private func sheet(_ stops: [Stop], isOnline: Bool, waypoints: Bool = true) -> some View {
        TripStopsSheet(model: TripStopsModel(
            trip: GalleryStops.trip(waypoints: waypoints), day: 0,
            finder: StopFinder(search: GalleryStopSearch(stops: stops)), isOnline: isOnline, onPick: { _ in }
        ))
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet).strokeBorder(OBCTheme.line))
    }
}

/// Apple Maps answers near Ulrichen on the sample line, and one waypoint a rider set.
enum GalleryStops {
    static let nearby = [
        Stop(name: "Hotel Walser", coordinate: Coordinate(latitude: 46.5054, longitude: 8.3128), kind: .hotel),
        Stop(name: "Hotel Astoria", coordinate: Coordinate(latitude: 46.5076, longitude: 8.3062), kind: .hotel),
        Stop(name: "Hotel Nufenen", coordinate: Coordinate(latitude: 46.5063, longitude: 8.3031), kind: .hotel),
        Stop(name: "Basegoms", coordinate: Coordinate(latitude: 46.5072, longitude: 8.3041), kind: .hotel),
        Stop(name: "camping riverside", coordinate: Coordinate(latitude: 46.4648, longitude: 8.2449), kind: .campsite),
        Stop(name: "Wald-Stellplatz Rhodania", coordinate: Coordinate(latitude: 46.4660, longitude: 8.2450), kind: .campsite),
    ]

    static let waypoint = Waypoint(
        index: 0, name: "Water · Obergesteln", distanceAlongMeters: 0,
        coordinate: Coordinate(latitude: 46.5147, longitude: 8.3242))

    /// The sample line as one file, the way "Start a trip" hands it to the day editor.
    static func oneFile() -> Trip {
        let points = SampleLine.alps.vertices.map { RoutePoint(coordinate: $0.coordinate, elevationMeters: $0.elevation) }
        return Trip.joining(
            [points], names: ["Alps traverse"], waypoints: [[waypoint]], id: TripID("gallery-file"),
            name: "Alps traverse", bikeType: .gravel, now: Date(timeIntervalSince1970: 0)
        ).withPlaceName("Brig", day: 0)
    }

    /// The sample line as two days, the first ending at Ulrichen.
    static func trip(waypoints: Bool) -> Trip {
        let points = SampleLine.alps.vertices.map {
            RoutePoint(coordinate: $0.coordinate, elevationMeters: $0.elevation)
        }
        let ulrichen = 20
        return Trip.joining(
            [Array(points[...ulrichen]), Array(points[ulrichen...])], names: [nil, "Goms"],
            waypoints: waypoints ? [[waypoint]] : [], id: TripID("gallery"), name: "Alps traverse",
            bikeType: .gravel, now: Date(timeIntervalSince1970: 0)
        ).withPlaceName("Ulrichen", day: 0)
    }
}

private extension Trip {
    func withPlaceName(_ name: String, day: Int) -> Trip {
        var trip = self
        trip.namePlace(day, to: name)
        return trip
    }
}

struct GalleryStopSearch: StopSearch {
    let stops: [Stop]

    func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        stops.filter { $0.coordinate.distance(to: center) <= radius }
    }

    func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop] {
        stops.filter { $0.name.localizedCaseInsensitiveContains(query) }
    }
}
#endif
