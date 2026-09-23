#if DEBUG
import Foundation
import OBCDomain

/// Apple Maps search without a network: it answers from a fixed list of stops and counts the
/// requests it gets. The default list is real Apple Maps places near the `trips` fixture.
public actor MockStopSearch: StopSearch {
    public static let fixtureStops = [
        Stop(
            name: "Devil's Lake State Park Campgrounds", coordinate: Coordinate(latitude: 43.4273, longitude: -89.7308),
            kind: .campsite, mapItemID: "IE76495F97B5E1726"),
        Stop(
            name: "Thunderbird Motor Inn", coordinate: Coordinate(latitude: 43.4752, longitude: -89.7260),
            kind: .hotel, mapItemID: "I39CC6FD0954529BD"),
        Stop(
            name: "Spinning Wheel Motel", coordinate: Coordinate(latitude: 43.4750, longitude: -89.7294),
            kind: .hotel, mapItemID: "I48D742ADE26829B5"),
    ]

    private let list: [Stop]
    /// Every request fails as it does without a connection.
    private let isOffline: Bool
    public private(set) var requestCount = 0

    public init(stops: [Stop] = MockStopSearch.fixtureStops, isOffline: Bool = false) {
        list = stops
        self.isOffline = isOffline
    }

    public func stops(near center: Coordinate, radius: Double) async throws -> [Stop] {
        try answer()
        return list.filter { $0.kind != .place && $0.coordinate.distance(to: center) <= radius }
    }

    public func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop] {
        try answer()
        return list.filter { $0.name.localizedCaseInsensitiveContains(query) }
    }

    private func answer() throws {
        requestCount += 1
        if isOffline { throw URLError(.notConnectedToInternet) }
    }
}
#endif
