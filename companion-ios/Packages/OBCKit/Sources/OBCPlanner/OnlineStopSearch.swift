import Foundation
import OBCDomain

public struct OnlineStopSearch: StopSearch {
    private let source: any PlannerDataSource
    public init(source: any PlannerDataSource = PlannerService.shared) { self.source = source }

    public func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        let dy = radius / 111_200, dx = dy / max(0.01, cos(center.latitude * .pi / 180))
        var query = PlannerSearchQuery(text: "sleep", view: [center.longitude - dx, center.latitude - dy,
                                                            center.longitude + dx, center.latitude + dy])
        query.kinds = ["lodging", "campsite"]
        return try await search(query).filter { center.distance(to: $0.coordinate) <= radius }
    }
    public func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop] {
        try await search(PlannerSearchQuery(text: query, view: [southWest.longitude, southWest.latitude,
                                                               northEast.longitude, northEast.latitude]))
    }
    private func search(_ query: PlannerSearchQuery) async throws -> [Stop] {
        let release = try await source.release()
        return try await source.search(query, release: release).map { place in
            let kind: Stop.Kind = switch place.kind {
            case "campsite": .campsite
            case "hotel", "hostel", "guest_house", "motel", "hut": .hotel
            default: .place
            }
            return Stop(name: place.name, coordinate: place.coordinate, kind: kind, mapItemID: place.source)
        }
    }
}
