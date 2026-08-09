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
    /// Travel bearing in meteorological degrees, or `nil` when the device does not vouch for it.
    public var bearingDegrees: Double?
    /// Ground speed in metres per second, or `nil` when the device does not vouch for it.
    public var speedMetresPerSecond: Double?
    /// Coordinates of the active route ahead of the rider, when one is being navigated. Used to
    /// widen the corridor along the actual road rather than a bearing cone.
    public var routeAhead: [Coordinate]
    /// Ground altitude in metres, when known — MET wants it to pick the right model level.
    public var altitudeMetres: Int?

    public init(
        requestID: UInt32 = 0,
        position: Coordinate? = nil,
        fixTime: Date? = nil,
        bearingDegrees: Double? = nil,
        speedMetresPerSecond: Double? = nil,
        routeAhead: [Coordinate] = [],
        altitudeMetres: Int? = nil
    ) {
        self.requestID = requestID
        self.position = position
        self.fixTime = fixTime
        self.bearingDegrees = bearingDegrees
        self.speedMetresPerSecond = speedMetresPerSecond
        self.routeAhead = routeAhead
        self.altitudeMetres = altitudeMetres
    }
}

/// The region a weather job must be able to answer for: the rider's position plus where two hours
/// of riding can plausibly take them.
///
/// The corridor is the **only** locality signal that ever reaches OBC infrastructure, and it reaches
/// it as tile indexes inside Range headers — never as a coordinate in a URL. MET is the one third
/// party that receives an actual rider coordinate, and that is a WX1 decision recorded in the
/// privacy declaration WX13 surfaces.
public struct WeatherCorridor: Equatable, Sendable {
    /// How far ahead the rain map has to reach; the epic's two-hour question.
    public static let horizon: TimeInterval = 2 * 3_600
    /// Sideways slack around the projected track: GPS error, a route that bends, and the fact that
    /// a rain cell has width.
    public static let lateralMarginMetres: Double = 5_000
    /// The smallest corridor, used when the device vouches for neither bearing nor speed. A rider
    /// who might go any direction gets a disc, not a fabricated heading.
    public static let minimumRadiusMetres: Double = 10_000
    /// Ceiling on the projected reach, so an implausible speed cannot turn into a continental
    /// corridor and a thousand tile reads.
    public static let maximumRadiusMetres: Double = 120_000

    public var bounds: WeatherBoundingBox
    /// True when neither bearing nor speed was trustworthy, so the corridor is an undirected disc.
    public var isUndirected: Bool

    public init(bounds: WeatherBoundingBox, isUndirected: Bool) {
        self.bounds = bounds
        self.isUndirected = isUndirected
    }

    /// Project the corridor for `request`, or `nil` when there is no position to project from.
    ///
    /// Three inputs, in order of trust: the route ahead (real geometry the rider intends to follow),
    /// the bearing/speed cone (what the device measured), and — when neither is vouched for — a
    /// plain disc. Every branch ends in `union` with the position disc, so the rider's own cell is
    /// always inside the corridor even at a standstill.
    public static func projected(for request: WeatherRequest) -> WeatherCorridor? {
        guard let position = request.position, position.isValidGeographic else { return nil }
        let latitude = position.latitudeMicrodegrees
        let longitude = position.longitudeMicrodegrees

        let reach = request.speedMetresPerSecond.map { speed in
            Swift.min(maximumRadiusMetres, Swift.max(minimumRadiusMetres, speed * horizon))
        }
        let directed = reach != nil && request.bearingDegrees != nil
        var bounds = WeatherBoundingBox.around(
            latitudeMicrodegrees: latitude, longitudeMicrodegrees: longitude,
            metres: directed ? lateralMarginMetres : (reach ?? minimumRadiusMetres))

        if directed, let reach, let bearing = request.bearingDegrees, bearing.isFinite {
            // Sample the great-circle-ish track ahead rather than only its endpoint: at high
            // latitudes the straight-line box would miss the middle of a curving path.
            let radians = bearing * .pi / 180
            let cosine = Swift.max(0.05, Foundation.cos(position.latitude * .pi / 180))
            for step in 1...8 {
                let distance = reach * Double(step) / 8
                let north = distance * Foundation.cos(radians)
                let east = distance * Foundation.sin(radians)
                let point = WeatherBoundingBox.around(
                    latitudeMicrodegrees: latitude + Int64((north / 111_320 * 1_000_000).rounded()),
                    longitudeMicrodegrees: longitude
                        + Int64((east / (111_320 * cosine) * 1_000_000).rounded()),
                    metres: lateralMarginMetres)
                bounds = bounds.union(point)
            }
        }

        // The route the rider is actually on beats any projection of it. Only the stretch inside
        // the reach is added — a 300 km route must not turn into a 300 km corridor.
        if !request.routeAhead.isEmpty {
            let limit = reach ?? minimumRadiusMetres
            var travelled = 0.0
            var previous = position
            for point in request.routeAhead where point.isValidGeographic {
                travelled += previous.distance(to: point)
                previous = point
                if travelled > limit { break }
                bounds = bounds.union(WeatherBoundingBox.around(
                    latitudeMicrodegrees: point.latitudeMicrodegrees,
                    longitudeMicrodegrees: point.longitudeMicrodegrees,
                    metres: lateralMarginMetres))
            }
        }

        // v1 grids never cross the antimeridian, so a corridor that would is clamped to the
        // hemisphere the rider is in rather than wrapped into a window meaning the other side of
        // the planet. The lost sliver reads as "not covered", which is honest.
        if bounds.westMicrodegrees < -180_000_000 { bounds.westMicrodegrees = -180_000_000 }
        if bounds.eastMicrodegrees > 180_000_000 { bounds.eastMicrodegrees = 180_000_000 }
        return WeatherCorridor(bounds: bounds, isUndirected: !directed)
    }
}
