import Foundation
import OBCDomain

/// A geographic window in integer microdegrees — the one coordinate currency of the weather path.
///
/// Everything downstream of this type (product bbox tests, grid cell lookup, the OBCW header) is
/// exact integer arithmetic in microdegrees, because both frozen contracts are: the manifest states
/// bboxes in microdegrees and OBCG/OBCW store `int32` microdegree edges plus microdegree strides.
/// Doing the corridor maths in `Double` degrees and rounding at the end would put the crop window a
/// cell off the source lattice, which is exactly the fabricated sub-cell precision the epic forbids.
///
/// The window is inclusive of `south`/`west` and exclusive of `north`/`east`, matching OBCG §3 and
/// OBCW §5. It never crosses the antimeridian: v1 grids do not, so a corridor that would is clamped
/// (see ``WeatherCorridor``) rather than silently wrapped into a window that means the far side of
/// the planet.
public struct WeatherBoundingBox: Equatable, Sendable {
    public var southMicrodegrees: Int64
    public var westMicrodegrees: Int64
    public var northMicrodegrees: Int64
    public var eastMicrodegrees: Int64

    public init(
        southMicrodegrees: Int64, westMicrodegrees: Int64,
        northMicrodegrees: Int64, eastMicrodegrees: Int64
    ) {
        self.southMicrodegrees = southMicrodegrees
        self.westMicrodegrees = westMicrodegrees
        self.northMicrodegrees = northMicrodegrees
        self.eastMicrodegrees = eastMicrodegrees
    }

    /// True when this box fully contains `other`. Product selection uses **containment**, never
    /// overlap: a product that covers half the corridor cannot answer the corridor's question, and
    /// pretending otherwise is how a rider gets a rain map that stops at an invisible border.
    public func contains(_ other: WeatherBoundingBox) -> Bool {
        other.southMicrodegrees >= southMicrodegrees && other.northMicrodegrees <= northMicrodegrees
            && other.westMicrodegrees >= westMicrodegrees && other.eastMicrodegrees <= eastMicrodegrees
    }

    public func contains(latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64) -> Bool {
        latitudeMicrodegrees >= southMicrodegrees && latitudeMicrodegrees <= northMicrodegrees
            && longitudeMicrodegrees >= westMicrodegrees && longitudeMicrodegrees <= eastMicrodegrees
    }

    public var isWellFormed: Bool {
        southMicrodegrees < northMicrodegrees && westMicrodegrees < eastMicrodegrees
            && southMicrodegrees >= -90_000_000 && northMicrodegrees <= 90_000_000
            && westMicrodegrees >= -180_000_000 && eastMicrodegrees <= 180_000_000
    }

    public func union(_ other: WeatherBoundingBox) -> WeatherBoundingBox {
        WeatherBoundingBox(
            southMicrodegrees: Swift.min(southMicrodegrees, other.southMicrodegrees),
            westMicrodegrees: Swift.min(westMicrodegrees, other.westMicrodegrees),
            northMicrodegrees: Swift.max(northMicrodegrees, other.northMicrodegrees),
            eastMicrodegrees: Swift.max(eastMicrodegrees, other.eastMicrodegrees))
    }

    /// The box around one coordinate, grown by `metres` in every direction.
    ///
    /// Longitude degrees shrink with latitude, so the east/west growth divides by `cos(latitude)`;
    /// near the poles that blows up, hence the clamp on the cosine. This is a *request corridor*,
    /// not a rendering projection — being slightly generous costs one more tile read, while being
    /// short would drop rain the rider is about to ride into.
    public static func around(
        latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64, metres: Double
    ) -> WeatherBoundingBox {
        let latitudeDegrees = Double(latitudeMicrodegrees) / 1_000_000
        let latitudeSpan = Int64((metres / 111_320 * 1_000_000).rounded(.up))
        let cosine = Swift.max(0.05, Foundation.cos(latitudeDegrees * .pi / 180))
        let longitudeSpan = Int64((metres / (111_320 * cosine) * 1_000_000).rounded(.up))
        return WeatherBoundingBox(
            southMicrodegrees: Swift.max(-90_000_000, latitudeMicrodegrees - latitudeSpan),
            westMicrodegrees: Swift.max(-180_000_000, longitudeMicrodegrees - longitudeSpan),
            northMicrodegrees: Swift.min(90_000_000, latitudeMicrodegrees + latitudeSpan),
            eastMicrodegrees: Swift.min(180_000_000, longitudeMicrodegrees + longitudeSpan))
    }
}

public extension Coordinate {
    /// Microdegrees, rounded to nearest — the wire's coordinate unit.
    var latitudeMicrodegrees: Int64 { Int64((latitude * 1_000_000).rounded()) }
    var longitudeMicrodegrees: Int64 { Int64((longitude * 1_000_000).rounded()) }
}
