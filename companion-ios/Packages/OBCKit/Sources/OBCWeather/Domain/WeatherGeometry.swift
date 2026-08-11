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

    public func contains(latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64) -> Bool {
        latitudeMicrodegrees >= southMicrodegrees && latitudeMicrodegrees <= northMicrodegrees
            && longitudeMicrodegrees >= westMicrodegrees && longitudeMicrodegrees <= eastMicrodegrees
    }

    public var isWellFormed: Bool {
        southMicrodegrees < northMicrodegrees && westMicrodegrees < eastMicrodegrees
            && southMicrodegrees >= -90_000_000 && northMicrodegrees <= 90_000_000
            && westMicrodegrees >= -180_000_000 && eastMicrodegrees <= 180_000_000
    }

    /// The window check the manifest reader applies before any shard arithmetic — the Swift twin of
    /// `Bbox::validate`.
    ///
    /// Looser than ``isWellFormed`` in exactly one place, and deliberately: `west > east` is not
    /// malformed here, it **means the window crosses the antimeridian** (OBCG §10.2). Every other
    /// spelling of that idea — a 0..360 longitude, a west below -180, an east above 180 — is
    /// ``WeatherBboxError/outOfRange``, because a client that silently reinterprets one answers a
    /// corridor from the wrong hemisphere with no error anywhere.
    public func validateAsWindow() throws {
        guard (-90_000_000...90_000_000).contains(southMicrodegrees),
              (-90_000_000...90_000_000).contains(northMicrodegrees),
              (-180_000_000...180_000_000).contains(westMicrodegrees),
              (-180_000_000...180_000_000).contains(eastMicrodegrees)
        else { throw WeatherBboxError.outOfRange }
        guard southMicrodegrees < northMicrodegrees, westMicrodegrees != eastMicrodegrees else {
            throw WeatherBboxError.empty
        }
    }

    /// The box around one coordinate, grown by `metres` in every direction — the Swift twin of
    /// `Bbox::around`, and bit-identical to it by construction.
    ///
    /// Longitude degrees shrink with latitude, so the east/west growth divides by `cos(latitude)`;
    /// near the poles that blows up, hence the clamp on the cosine. Spans round **outward** (`ceil`)
    /// in integer microdegrees: this is a *request corridor*, not a rendering projection — being
    /// slightly generous costs one more tile read, while being short would drop rain the rider is
    /// about to ride into.
    ///
    /// **The two clamps are the honest edges of the disc, not an afterthought.** `OBCW_Spec.md` §1
    /// forbids a bundle window crossing the antimeridian, so a disc that reaches past the date line
    /// is cut there and the sliver beyond reads as not-covered; near a pole the latitude clamps, and
    /// `covered_rows` then makes the plan answer ``WeatherPlanOutcome/uncovered`` rather than
    /// producing an illegal window. Full antimeridian *wrap* support still lives in the manifest
    /// reader, where a `west > east` bbox is served by splitting — it is the corridor, not the
    /// lattice, that refuses to wrap.
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
