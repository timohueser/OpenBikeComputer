import Foundation
import OBCDomain

/// One weather job's inputs, provider-agnostic and transport-agnostic.
///
/// This is deliberately **not** `WeatherRequestContext` (the 52 BLE bytes). WX9 owns the link and
/// maps a context read into this value; the weather domain must be drivable from a fixture, a
/// simulator or a test with no CoreBluetooth in the process at all. The mapping is also where the
/// wire's validity flags become Swift optionals: a cleared `.position` bit becomes `position == nil`
/// here, never latitude 0.
public struct WeatherRequest: Equatable, Sendable {
    /// The device's request nonce, stamped into the OBCW header so the two BLE connections
    /// correlate. `0` is legal and means unsolicited/manual material (OBCW §3).
    public var requestID: UInt32
    /// Where the rider is. `nil` when the device had no usable fix — there is then nothing to ask
    /// either provider about, and the job fails rather than guessing a location.
    public var position: Coordinate?
    /// When that fix was taken.
    public var fixTime: Date?
    /// Ground altitude in metres, when known — MET wants it to pick the right model level.
    public var altitudeMetres: Int?

    public init(
        requestID: UInt32 = 0,
        position: Coordinate? = nil,
        fixTime: Date? = nil,
        altitudeMetres: Int? = nil
    ) {
        self.requestID = requestID
        self.position = position
        self.fixTime = fixTime
        self.altitudeMetres = altitudeMetres
    }
}

/// The region a weather job must be able to answer for: **a plain 90 km disc around the rider**.
///
/// No projection, no bearing cone, no route sampling, no speed (#1244). The old corridor existed to
/// keep a *heterogeneous* set of products' bboxes small enough to be worth fetching; with one uniform
/// lattice the question is only "which shards", and 90 km is chosen so that 30 km of riding inside
/// the two-hour horizon never leaves the window. A disc is also the only honest shape when the
/// device's bearing is a measurement and the route is a plan: both were inputs the corridor could
/// only ever be wrong about, and neither bought coverage the disc does not already have.
///
/// The corridor is the **only** locality signal that ever reaches OBC infrastructure, and it reaches
/// it as tile indexes inside Range headers — never as a coordinate in a URL. MET is the one third
/// party that receives an actual rider coordinate, and that is a WX1 decision recorded in the
/// privacy declaration WX13 surfaces.
public struct WeatherCorridor: Equatable, Sendable {
    /// How far ahead the rain map has to reach; the epic's two-hour question, and the reason 90 km
    /// is the radius. **Dataset-level, not a product policy** — which is why it survived #1244 while
    /// the projection constants did not, and why `host/obc-wx-client` keeps its twin `HORIZON_S`.
    /// The timeline's actual depth is the manifest's `cadence`, read per document; this states the
    /// question both clients are sizing for, and ``OBCWeatherServiceClient`` filters the planned
    /// frames against it so a manifest publishing further out costs no Range reads.
    public static let horizon: TimeInterval = 2 * 3_600
    /// How old a frame may be and still be worth fetching — the twin of `MAX_OBSERVATION_AGE_S`.
    /// Past it a "current" frame would be a lie told with a true timestamp, so it is not fetched and
    /// the rider is told the published frames are outside the window rather than shown stale rain.
    public static let maximumObservationAge: TimeInterval = 6 * 3_600
    /// The disc's radius. One number, no policy.
    public static let corridorRadiusMetres: Double = 90_000

    public var bounds: WeatherBoundingBox

    public init(bounds: WeatherBoundingBox) {
        self.bounds = bounds
    }

    /// The disc around `request`'s position, or `nil` when there is no position to centre it on.
    public static func around(_ request: WeatherRequest) -> WeatherCorridor? {
        guard let position = request.position, position.isValidGeographic else { return nil }
        return WeatherCorridor(bounds: WeatherBoundingBox.around(
            latitudeMicrodegrees: position.latitudeMicrodegrees,
            longitudeMicrodegrees: position.longitudeMicrodegrees,
            metres: corridorRadiusMetres))
    }
}
