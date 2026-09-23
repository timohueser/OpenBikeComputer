import Foundation

/// The line operations of a ``Trip``: join files, reverse, re-project the day ends, and cut the
/// line into day routes. Every change to the line ends in ``Trip/reproject()``, so a day end
/// stays at its place and only its distance moves.
extension Trip {
    /// A day end farther than this from the changed line is dropped.
    public static let dayEndDropMeters = 500.0
    /// The shortest day. A file shorter than this is not a day, and a projection closer than
    /// this to the previous day end or to the line end would make one, so that day end is
    /// dropped.
    public static let minimumDayMeters = 100.0
    /// Two waypoints with one name this close are one stop: files that meet often both carry
    /// the place where they meet.
    public static let sameWaypointMeters = 10.0
    /// The window of the second, local projection pass. It re-centres the planar maths on the
    /// candidate, so the drop check measures true metres on a long line.
    static let refineWindowMeters = 2_000.0

    /// A new trip from route files in ride order: one day per file, the day ends on the file
    /// boundaries. A file shorter than ``minimumDayMeters`` adds nothing.
    /// `names` gives each file's day its own name, and `waypoints` each file's waypoints, by
    /// index; a missing or blank name leaves the day to "Day N ‹place›".
    public static func joining(
        _ files: [[RoutePoint]], names: [String?] = [], waypoints: [[Waypoint]] = [],
        id: TripID, key: UInt64 = Trip.newKey(), name: String, bikeType: BikeType, now: Date
    ) -> Trip {
        var trip = Trip(id: id, key: key, name: name, bikeType: bikeType, addedAt: now)
        for (index, file) in files.enumerated() {
            trip.append(
                file, name: index < names.count ? names[index] : nil,
                waypoints: index < waypoints.count ? waypoints[index] : [])
        }
        return trip
    }

    /// The line measured with its gaps, as the marker control and the cut read it.
    public var measuredLine: MeasuredLine {
        MeasuredLine(
            coordinates: line.map(\.coordinate), elevations: line.map(\.elevationMeters),
            pieceStarts: pieceStarts)
    }

    /// Whether `file` is long enough to be a day.
    public static func isDay(_ file: [RoutePoint]) -> Bool {
        file.count > 1 && MeasuredLine(routePoints: file).length >= minimumDayMeters
    }

    /// Add a file as a new last day. The old line end becomes a day end. A file that starts
    /// exactly where the line ends continues the piece; any other start opens a new piece, so
    /// the gap sits at the day end and the next day starts where the file starts. A file that is
    /// not a day changes nothing. The file's waypoints become stops of the trip, once each.
    /// Returns the day ends the change dropped.
    @discardableResult
    public mutating func append(_ file: [RoutePoint], name: String? = nil, waypoints: [Waypoint] = []) -> [DayEnd] {
        guard Self.isDay(file) else { return [] }
        for waypoint in waypoints.map(Stop.init(waypoint:)) where !self.waypoints.contains(where: {
            $0.name == waypoint.name && $0.coordinate.distance(to: waypoint.coordinate) <= Self.sameWaypointMeters
        }) {
            self.waypoints.append(waypoint)
        }
        var points = file
        if let end = line.last {
            if points[0].coordinate == end.coordinate {
                points.removeFirst()
            } else {
                pieceStarts.append(line.count)
            }
        }
        line += points
        dayEnds.append(DayEnd(
            coordinate: points[points.count - 1].coordinate, title: Self.trimmed(name), distance: 0))
        return reproject()
    }

    /// Reverse the whole trip: the direction and the order of the days. Every day end keeps its
    /// place and its place name; the start and the last day end swap place names. A day end at a
    /// transfer keeps its boundary and its transfer: it moves to the other side of the gap, and
    /// the two place names of the boundary swap with it. Every day keeps its own name. The trip
    /// gets a new key, so device progress of the old direction does not carry over. Returns the
    /// day ends the change dropped.
    @discardableResult
    public mutating func reverse() -> [DayEnd] {
        guard line.count > 1 else { return [] }
        let length = measuredLine.length
        let count = line.count
        // Where the next day starts, for each day end at a transfer.
        let resumes = dayEnds.indices.map { transferStart(after: $0).map { line[$0].coordinate } }
        line = ImportedRoute(points: line).reversed().points
        // A gap into point i lies between i-1 and i; reversed, it leads into point count-i.
        pieceStarts = pieceStarts.map { count - $0 }.sorted()
        // Old day k becomes day n-1-k, and now ends where it used to start.
        let titles = Array(dayEnds.map(\.title).reversed())
        let interior = zip(dayEnds, resumes).dropLast().reversed().enumerated().map { day, pair in
            let (end, resume) = pair
            guard let resume else {
                return DayEnd(
                    coordinate: end.coordinate, name: end.name, title: titles[day], distance: length - end.distance,
                    stop: end.stop, stopRoute: end.stopRoute?.reversed(length: length))
            }
            return DayEnd(
                coordinate: resume, name: end.resumeName, title: titles[day], distance: length - end.distance,
                transfer: end.transfer, resumeName: end.name)
        }
        let endName = dayEnds.last?.name
        dayEnds = interior + [DayEnd(
            coordinate: line[count - 1].coordinate, name: startName, title: titles.last ?? nil, distance: length)]
        startName = endName
        key = Self.newKey()
        return reproject()
    }

    /// Project every day end onto the line again, near its stored distance. The last day end
    /// follows the line end. A day end farther than ``dayEndDropMeters`` from the line, or one
    /// that would make an empty day, is dropped; the dropped ends come back so the rider can be
    /// told which went.
    @discardableResult
    public mutating func reproject() -> [DayEnd] {
        guard let end = line.last else {
            dayEnds = []
            return []
        }
        let measured = measuredLine
        let length = measured.length
        var kept: [DayEnd] = []
        var dropped: [DayEnd] = []
        var previous = 0.0
        for var dayEnd in dayEnds.dropLast() {
            let coarse = measured.projection(of: dayEnd.coordinate, near: dayEnd.distance, window: length)
            let fine = measured.projection(of: dayEnd.coordinate, near: coarse.distance, window: Self.refineWindowMeters)
            guard fine.error <= Self.dayEndDropMeters,
                fine.distance >= previous + Self.minimumDayMeters,
                fine.distance <= length - Self.minimumDayMeters
            else {
                dropped.append(dayEnd)
                continue
            }
            dayEnd.stopRoute = dayEnd.stopRoute?.reprojected(
                on: measured, old: dayEnd.distance, new: fine.distance, error: fine.error)
            dayEnd.distance = fine.distance
            kept.append(dayEnd)
            previous = fine.distance
        }
        kept.append(DayEnd(
            coordinate: end.coordinate, name: dayEnds.last?.name, title: dayEnds.last?.title, distance: length))
        dayEnds = kept
        return dropped
    }

    // MARK: Cut

    /// The line cut at the day ends: one route per day, in ride order. A day that starts at a
    /// gap starts at the next piece; a gap inside a day stays in it as a straight segment. The
    /// stop routes are inlined: a day ends with the spur out or the via leg to its stop, and the
    /// next day starts with the spur back or the via leg from it.
    public func dayLines() -> [DayLine] {
        guard line.count > 1 else { return [] }
        let measured = measuredLine
        return dayEnds.indices.map { day in
            let previous = day > 0 ? dayEnds[day - 1].stopRoute : nil
            var cut = DayLine(points: [], joinIndex: 0, leaveIndex: nil)
            switch previous {
            case .outAndBack(let spur)?: cut.append(spur.reversed())
            case .via(_, let fromStop, _, _)?: cut.append(fromStop)
            case nil: break
            }
            let main = cut.append(slice(measured, from: lineStart(of: day), to: lineEnd(of: day)))
            if case .outAndBack? = previous { cut.joinIndex = main }
            switch dayEnds[day].stopRoute {
            case .outAndBack(let spur)?:
                cut.leaveIndex = cut.points.count - 1
                cut.append(spur)
            case .via(let toStop, _, _, _)?: cut.append(toStop)
            case nil: break
            }
            return cut
        }
    }

    private func slice(_ measured: MeasuredLine, from: Double, to: Double) -> [RoutePoint] {
        let vertices = measured.vertices
        var first = measured.index(at: from)
        var points: [RoutePoint]
        if first + 1 < line.count, pieceStarts.contains(first + 1), vertices[first + 1].distance <= from {
            first += 1
            points = [line[first]]
        } else {
            points = [point(on: measured, segment: first, at: from)]
        }
        var next = first + 1
        while next < line.count, vertices[next].distance < to {
            if vertices[next].distance > from, line[next].coordinate != points[points.count - 1].coordinate {
                points.append(line[next])
            }
            next += 1
        }
        let last = point(on: measured, segment: measured.index(at: to), at: to)
        if last.coordinate != points[points.count - 1].coordinate { points.append(last) }
        return points
    }

    /// The point `distance` metres along the line, on the segment that starts at `segment`. A
    /// point that is not a vertex takes the surface and the elevation state of its segment.
    private func point(on measured: MeasuredLine, segment i: Int, at distance: Double) -> RoutePoint {
        guard i + 1 < line.count else { return line[i] }
        let a = line[i], b = line[i + 1]
        let start = measured.vertices[i].distance
        let span = measured.vertices[i + 1].distance - start
        let t = span > 0 ? (distance - start) / span : 0
        if t <= 1e-9 { return a }
        if t >= 1 - 1e-9 { return b }
        let elevation: Double? =
            if let ea = a.elevationMeters, let eb = b.elevationMeters { ea + (eb - ea) * t } else { nil }
        return RoutePoint(
            coordinate: Coordinate(
                latitude: a.coordinate.latitude + (b.coordinate.latitude - a.coordinate.latitude) * t,
                longitude: a.coordinate.longitude + (b.coordinate.longitude - a.coordinate.longitude) * t),
            elevationMeters: elevation,
            surface: b.surface,
            elevationIncomplete: b.elevationIncomplete)
    }
}

/// One day's route as an upload cuts it from the trip line.
public struct DayLine: Equatable, Sendable {
    public var points: [RoutePoint]
    /// The first point on the main line: after the spur back of an out and back, else 0.
    public var joinIndex: Int
    /// The last point on the main line, when the spur out of an out and back follows it.
    public var leaveIndex: Int?

    /// Append `more`, without a first point that repeats the last one. Returns the index of
    /// `more`'s first point.
    @discardableResult
    mutating func append(_ more: [RoutePoint]) -> Int {
        guard let first = more.first else { return points.count }
        if points.last?.coordinate == first.coordinate {
            points += more.dropFirst()
            return points.count - more.count
        }
        points += more
        return points.count - more.count
    }
}
