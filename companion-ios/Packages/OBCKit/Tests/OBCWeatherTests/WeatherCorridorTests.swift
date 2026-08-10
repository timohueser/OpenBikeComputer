import Foundation
import OBCDomain
import Testing
@testable import OBCWeather

/// The corridor is the only locality signal that reaches OBC infrastructure, so what it contains —
/// and what it refuses to invent — matters.
///
/// Since #1244 it is a plain 90 km disc: no bearing cone, no speed, no route sampling. The six
/// projection cases this suite used to carry went with the projection, and nothing replaced them,
/// because there is nothing left to get wrong about *shape*. What is left is the arithmetic — the
/// integer span, the cosine clamp, and the two edge clamps the disc owes `OBCG_Spec` §1.
struct WeatherCorridorTests {
    static let position = Coordinate(latitude: 47.2, longitude: 7.3)

    @Test
    func noPositionMeansNoCorridorRatherThanTheEquator() {
        #expect(WeatherCorridor.around(WeatherRequest(requestID: 1)) == nil)
        #expect(WeatherCorridor.around(
            WeatherRequest(position: Coordinate(latitude: .nan, longitude: 0))) == nil)
    }

    /// A disc, centred on the rider, 90 km in every direction.
    @Test
    func theCorridorIsA90KilometreDiscAroundTheRider() throws {
        let corridor = try #require(
            WeatherCorridor.around(WeatherRequest(position: Self.position)))
        let bounds = corridor.bounds
        #expect(bounds.contains(
            latitudeMicrodegrees: 47_200_000, longitudeMicrodegrees: 7_300_000))
        // Symmetric about the rider on both axes.
        #expect(bounds.northMicrodegrees - 47_200_000 == 47_200_000 - bounds.southMicrodegrees)
        #expect(bounds.eastMicrodegrees - 7_300_000 == 7_300_000 - bounds.westMicrodegrees)
        // 90 km is 0.8085 degrees of latitude, rounded outward to whole microdegrees.
        #expect(bounds.northMicrodegrees - 47_200_000 == 808_481)
        // A degree of longitude is shorter at 47.2 N, so the east-west span is wider in degrees.
        let cosine = Foundation.cos(47.2 * .pi / 180)
        let expectedLongitudeSpan = Int64((90_000 / (111_320 * cosine) * 1_000_000).rounded(.up))
        #expect(bounds.eastMicrodegrees - 7_300_000 == expectedLongitudeSpan)
    }

    /// Nothing the device measured changes the disc. Bearing and speed still ride the wire (§11.2)
    /// and still show up in diagnostics; they simply do not reach the corridor any more.
    @Test
    func theDiscDoesNotDependOnAnythingTheDeviceMeasured() throws {
        let bare = try #require(WeatherCorridor.around(WeatherRequest(position: Self.position)))
        let withFix = try #require(WeatherCorridor.around(WeatherRequest(
            requestID: 7, position: Self.position, fixTime: Date(timeIntervalSince1970: 1),
            altitudeMetres: 340)))
        #expect(bare == withFix)
    }

    /// **The antimeridian clamp.** `OBCW_Spec` §1 forbids a bundle window crossing ±180, so the disc
    /// is cut at the date line and the sliver beyond reads as not-covered — honest, and legal. The
    /// manifest reader still wraps (`west > east`); it is the corridor that refuses to.
    @Test
    func theCorridorIsCutAtTheAntimeridianRatherThanWrapped() throws {
        let corridor = try #require(WeatherCorridor.around(
            WeatherRequest(position: Coordinate(latitude: -16.9, longitude: 179.98))))
        #expect(corridor.bounds.eastMicrodegrees == 180_000_000)
        #expect(corridor.bounds.westMicrodegrees < corridor.bounds.eastMicrodegrees,
                "cut, not wrapped: a window with west > east would be an illegal OBCW bundle")
        #expect(corridor.bounds.isWellFormed)
        try corridor.bounds.validateAsWindow()

        let western = try #require(WeatherCorridor.around(
            WeatherRequest(position: Coordinate(latitude: -16.9, longitude: -179.98))))
        #expect(western.bounds.westMicrodegrees == -180_000_000)
        #expect(western.bounds.isWellFormed)
    }

    /// **The pole clamp.** Latitude stops at ±90 rather than producing a window nothing can express;
    /// the disc is then a band across the top of the lattice, and `covered_rows` is what turns it
    /// into ``WeatherPlanOutcome/uncovered`` rather than nine Range reads for the word "unknown".
    @Test
    func theCorridorIsClampedAtThePoleRatherThanReachingPastIt() throws {
        let corridor = try #require(WeatherCorridor.around(
            WeatherRequest(position: Coordinate(latitude: 89.7, longitude: 20))))
        #expect(corridor.bounds.northMicrodegrees == 90_000_000)
        #expect(corridor.bounds.isWellFormed)
        try corridor.bounds.validateAsWindow()
        // The 0.05 cosine clamp keeps the longitudinal span finite rather than exploding past 360.
        #expect(corridor.bounds.eastMicrodegrees <= 180_000_000)
        #expect(corridor.bounds.westMicrodegrees >= -180_000_000)

        let southern = try #require(WeatherCorridor.around(
            WeatherRequest(position: Coordinate(latitude: -89.8, longitude: 20))))
        #expect(southern.bounds.southMicrodegrees == -90_000_000)
        #expect(southern.bounds.isWellFormed)
    }

    /// A window is only refused for the things a client must never silently repair: an out-of-range
    /// coordinate, and a window with no area. `west > east` is *not* one of them — it means the
    /// antimeridian, which the shard arithmetic serves by splitting.
    @Test
    func windowValidationRefusesTheThingsAClampWouldHide() {
        let zeroTo360 = WeatherBoundingBox(
            southMicrodegrees: 47_900_000, westMicrodegrees: 352_100_000,
            northMicrodegrees: 48_100_000, eastMicrodegrees: 352_200_000)
        #expect(throws: WeatherBboxError.outOfRange) { try zeroTo360.validateAsWindow() }

        let pastThePole = WeatherBoundingBox(
            southMicrodegrees: 89_990_000, westMicrodegrees: 7_750_000,
            northMicrodegrees: 90_500_000, eastMicrodegrees: 7_950_000)
        #expect(throws: WeatherBboxError.outOfRange) { try pastThePole.validateAsWindow() }

        let flat = WeatherBoundingBox(
            southMicrodegrees: 48_000_000, westMicrodegrees: 7_750_000,
            northMicrodegrees: 48_000_000, eastMicrodegrees: 7_950_000)
        #expect(throws: WeatherBboxError.empty) { try flat.validateAsWindow() }

        let wrapping = WeatherBoundingBox(
            southMicrodegrees: -17_100_000, westMicrodegrees: 179_900_000,
            northMicrodegrees: -16_900_000, eastMicrodegrees: -179_900_000)
        #expect(throws: Never.self) { try wrapping.validateAsWindow() }
    }
}
