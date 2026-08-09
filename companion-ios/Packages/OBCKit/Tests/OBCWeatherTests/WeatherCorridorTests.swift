import Foundation
import OBCDomain
import Testing
@testable import OBCWeather

/// The corridor is the only locality signal that reaches OBC infrastructure, so what it contains —
/// and what it refuses to invent — matters.
struct WeatherCorridorTests {
    static let position = Coordinate(latitude: 47.2, longitude: 7.3)

    @Test
    func noPositionMeansNoCorridorRatherThanTheEquator() {
        #expect(WeatherCorridor.projected(for: WeatherRequest(requestID: 1)) == nil)
        #expect(WeatherCorridor.projected(for: WeatherRequest(
            position: Coordinate(latitude: .nan, longitude: 0))) == nil)
    }

    /// Neither bearing nor speed vouched for: a disc, not a fabricated heading.
    @Test
    func anUntrustedBearingProducesAnUndirectedDisc() throws {
        let corridor = try #require(WeatherCorridor.projected(
            for: WeatherRequest(position: Self.position)))
        #expect(corridor.isUndirected)
        let bounds = corridor.bounds
        #expect(bounds.contains(
            latitudeMicrodegrees: 47_200_000, longitudeMicrodegrees: 7_300_000))
        // Symmetric about the rider, roughly the minimum radius in each direction.
        let northSpan = bounds.northMicrodegrees - 47_200_000
        let southSpan = 47_200_000 - bounds.southMicrodegrees
        #expect(northSpan == southSpan)
        #expect(northSpan > 85_000 && northSpan < 95_000, "10 km is about 0.09 degrees of latitude")
    }

    @Test
    func aTrustedBearingAndSpeedProjectTwoHoursAhead() throws {
        let corridor = try #require(WeatherCorridor.projected(for: WeatherRequest(
            position: Self.position, bearingDegrees: 0, speedMetresPerSecond: 8)))
        #expect(!corridor.isUndirected)
        // Two hours at 8 m/s is 57.6 km north; the corridor reaches it and stays narrow behind.
        #expect(corridor.bounds.northMicrodegrees > 47_200_000 + 500_000)
        #expect(47_200_000 - corridor.bounds.southMicrodegrees < 60_000)
    }

    @Test
    func anImplausibleSpeedCannotProduceAContinentalCorridor() throws {
        let corridor = try #require(WeatherCorridor.projected(for: WeatherRequest(
            position: Self.position, bearingDegrees: 90, speedMetresPerSecond: 400)))
        let eastSpan = Double(corridor.bounds.eastMicrodegrees - 7_300_000) / 1_000_000
        // Capped at 120 km, which near 47 N is well under two degrees of longitude.
        #expect(eastSpan < 2.0)
    }

    @Test
    func theRouteAheadWidensTheCorridorOnlyAsFarAsTheRiderCanGet() throws {
        // A route that turns hard east after the projected straight-line cone.
        let route = (1...40).map { step in
            Coordinate(latitude: 47.2, longitude: 7.3 + Double(step) * 0.02)
        }
        let withRoute = try #require(WeatherCorridor.projected(for: WeatherRequest(
            position: Self.position, bearingDegrees: 0, speedMetresPerSecond: 5,
            routeAhead: route)))
        let withoutRoute = try #require(WeatherCorridor.projected(for: WeatherRequest(
            position: Self.position, bearingDegrees: 0, speedMetresPerSecond: 5)))
        #expect(withRoute.bounds.eastMicrodegrees > withoutRoute.bounds.eastMicrodegrees)
        // But not to the end of a 60 km route: two hours at 5 m/s is 36 km.
        #expect(withRoute.bounds.eastMicrodegrees < 7_300_000 + 800_000)
    }

    @Test
    func theCorridorNeverCrossesTheAntimeridian() throws {
        let corridor = try #require(WeatherCorridor.projected(for: WeatherRequest(
            position: Coordinate(latitude: -16.9, longitude: 179.98),
            bearingDegrees: 90, speedMetresPerSecond: 10)))
        #expect(corridor.bounds.eastMicrodegrees <= 180_000_000)
        #expect(corridor.bounds.isWellFormed)
    }

    @Test
    func containmentIsStrictInBothDirections() {
        let outer = WeatherBoundingBox(
            southMicrodegrees: 0, westMicrodegrees: 0,
            northMicrodegrees: 1_000_000, eastMicrodegrees: 1_000_000)
        let inner = WeatherBoundingBox(
            southMicrodegrees: 100_000, westMicrodegrees: 100_000,
            northMicrodegrees: 900_000, eastMicrodegrees: 900_000)
        #expect(outer.contains(inner))
        #expect(!inner.contains(outer))
        #expect(outer.contains(outer), "the closed test is deliberate: an exact fit is covered")
    }
}
