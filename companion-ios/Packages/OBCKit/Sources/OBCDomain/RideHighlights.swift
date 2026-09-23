import Foundation

/// A notable part of a ride, computed from its points and never typed.
public enum RideHighlight: Equatable, Sendable {
    /// The highest point: its elevation, its distance along the ride, and the name of a known
    /// place there.
    case highestPoint(elevation: Double, distance: Double, place: String?)
    /// The longest continuous climb by ascent, with its length in metres.
    case longestClimb(ascent: Double, length: Double)
    /// The highest speed held on a downhill grade, in metres per second.
    case fastestDescent(speedMps: Double)
    /// The ride's trip day covers the most distance of its trip.
    case biggestDay(distance: Double)
}

/// The highlight rules. Distances run along `MeasuredLine`, so a pause gap counts nothing.
public enum RideHighlights {
    /// A dip smaller than this does not end a climb.
    public static let climbDipMeters = 20.0
    /// A descent speed must hold this long, because GPS noise inflates a single sample.
    public static let descentWindow: TimeInterval = 20
    /// The grade a descent window must reach. A flat sprint with barometer drift is not a descent.
    public static let descentGrade = -0.02
    /// A waypoint this near the highest point names it.
    public static let placeRadiusMeters = 200.0
    public static let limit = 3

    /// At most `limit` highlights, the most notable first. `places` are the named waypoints that
    /// can name the highest point. `library` is every ride summary; the ride's trip picks its own.
    public static func compute(
        _ ride: Ride, places: [Waypoint] = [], library: [RideSummary] = []
    ) -> [RideHighlight] {
        let line = MeasuredLine(ridePoints: ride.points)
        // Each candidate scores against a chosen reference for a big day in the mountains:
        // a 2,000 m high point, a 500 m climb, a 50 kph descent, a 100 km day. The floors (100 m
        // of relief, a 100 m climb, 36 kph, a trip of two days) keep a flat ride from celebrating
        // noise.
        var scored: [(highlight: RideHighlight, score: Double)] = []
        if let high = highestPoint(line, places: places), prominence(line) >= 100 {
            scored.append((.highestPoint(elevation: high.elevation, distance: high.distance, place: high.place),
                           high.elevation / 2_000))
        }
        if let climb = longestClimb(line), climb.ascent >= 100 {
            scored.append((.longestClimb(ascent: climb.ascent, length: climb.length), climb.ascent / 500))
        }
        if let speed = fastestDescent(ride.points, line: line), speed >= 10 {
            scored.append((.fastestDescent(speedMps: speed), speed * 3.6 / 50))
        }
        if let trip = ride.summary.trip,
            let day = biggestDay(library.filter { $0.trip?.key == trip.key }),
            day.dayCount > 1, day.dayIndex == trip.dayIndex {
            scored.append((.biggestDay(distance: day.distance), day.distance / 100_000))
        }
        return scored.sorted { $0.score > $1.score }.prefix(limit).map(\.highlight)
    }

    /// The highest vertex, named by the nearest place within `placeRadiusMeters`.
    public static func highestPoint(
        _ line: MeasuredLine, places: [Waypoint] = []
    ) -> (elevation: Double, distance: Double, place: String?)? {
        guard line.hasElevation, let top = line.vertices.max(by: { $0.elevation < $1.elevation }) else {
            return nil
        }
        let place = places
            .map { (name: $0.name, meters: $0.coordinate.routeDistance(to: top.coordinate)) }
            .filter { $0.meters <= placeRadiusMeters && !$0.name.isEmpty }
            .min { $0.meters < $1.meters }?.name
        return (top.elevation, top.distance, place)
    }

    /// The climb with the most ascent. A climb runs from its last low point to the highest point
    /// before the line drops `climbDipMeters` below it, or back to the climb's start.
    public static func longestClimb(_ line: MeasuredLine) -> (ascent: Double, length: Double)? {
        guard line.hasElevation, !line.vertices.isEmpty else { return nil }
        let v = line.vertices
        var best: (ascent: Double, length: Double)?
        var bottom = 0
        var top = 0
        func close() {
            let ascent = v[top].elevation - v[bottom].elevation
            if ascent > 0, ascent > best?.ascent ?? 0 {
                best = (ascent, v[top].distance - v[bottom].distance)
            }
        }
        for i in v.indices.dropFirst() {
            let e = v[i].elevation
            if e > v[top].elevation {
                top = i
            } else if e <= v[bottom].elevation || v[top].elevation - e >= climbDipMeters {
                close()
                bottom = i
                top = i
            }
        }
        close()
        return best
    }

    /// The highest average speed over the shortest window of at least `descentWindow` whose grade
    /// reaches `descentGrade`, in metres per second. A window never spans a pause gap.
    public static func fastestDescent(_ points: [RidePoint], line: MeasuredLine) -> Double? {
        guard line.hasElevation, points.count == line.vertices.count else { return nil }
        let v = line.vertices
        var best: Double?
        var j = 0
        var lastPieceStart = 0
        for i in points.indices {
            j = max(j, i)
            while j < points.count,
                points[j].timestamp.timeIntervalSince(points[i].timestamp) < descentWindow {
                j += 1
                if j < points.count, line.pieceStarts.contains(j) { lastPieceStart = j }
            }
            guard j < points.count else { break }
            guard lastPieceStart <= i else { continue }
            let run = v[j].distance - v[i].distance
            guard run > 0, (v[j].elevation - v[i].elevation) / run <= descentGrade else { continue }
            let speed = run / points[j].timestamp.timeIntervalSince(points[i].timestamp)
            if speed > best ?? 0 { best = speed }
        }
        return best
    }

    /// The day with the most distance, summed over the rides that started on it, with the number
    /// of days ridden. `rides` belong to one trip. Nil when none has a trip day. A tie goes to the
    /// earlier day.
    public static func biggestDay(_ rides: [RideSummary]) -> (dayIndex: Int, distance: Double, dayCount: Int)? {
        var days: [Int: Double] = [:]
        for ride in rides {
            guard let trip = ride.trip else { continue }
            days[trip.dayIndex, default: 0] += ride.distanceMeters
        }
        guard let day = days.max(by: { $0.value < $1.value || ($0.value == $1.value && $0.key > $1.key) })
        else { return nil }
        return (day.key, day.value, days.count)
    }

    private static func prominence(_ line: MeasuredLine) -> Double {
        let elevations = line.vertices.map(\.elevation)
        return (elevations.max() ?? 0) - (elevations.min() ?? 0)
    }
}
