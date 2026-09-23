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
    /// The window of the second, local projection pass. It re-centres the planar maths on the
    /// candidate, so the drop check measures true metres on a long line.
    static let refineWindowMeters = 2_000.0

    /// A new trip from route files in ride order: one day per file, the day ends on the file
    /// boundaries. A file shorter than ``minimumDayMeters`` adds nothing.
    public static func joining(
        _ files: [[RoutePoint]], id: TripID, name: String, bikeType: BikeType, now: Date
    ) -> Trip {
        var trip = Trip(id: id, name: name, bikeType: bikeType, addedAt: now)
        for file in files { trip.append(file) }
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
    /// not a day changes nothing. Returns the day ends the change dropped.
    @discardableResult
    public mutating func append(_ file: [RoutePoint]) -> [DayEnd] {
        guard Self.isDay(file) else { return [] }
        var points = file
        if let end = line.last {
            if points[0].coordinate == end.coordinate {
                points.removeFirst()
            } else {
                pieceStarts.append(line.count)
            }
        }
        line += points
        dayEnds.append(DayEnd(coordinate: points[points.count - 1].coordinate, distance: 0))
        return reproject()
    }

    /// Reverse the whole trip: the direction and the order of the days. Every day end keeps its
    /// place and its name; the start and the last day end swap names. The trip gets a new key,
    /// so device progress of the old direction does not carry over. Returns the day ends the
    /// change dropped.
    @discardableResult
    public mutating func reverse() -> [DayEnd] {
        guard line.count > 1 else { return [] }
        let length = measuredLine.length
        let count = line.count
        line = ImportedRoute(points: line).reversed().points
        // A gap into point i lies between i-1 and i; reversed, it leads into point count-i.
        pieceStarts = pieceStarts.map { count - $0 }.sorted()
        let interior = dayEnds.dropLast().reversed().map { end in
            DayEnd(coordinate: end.coordinate, name: end.name, distance: length - end.distance)
        }
        let endName = dayEnds.last?.name
        dayEnds = interior + [DayEnd(coordinate: line[count - 1].coordinate, name: startName, distance: length)]
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
            dayEnd.distance = fine.distance
            kept.append(dayEnd)
            previous = fine.distance
        }
        kept.append(DayEnd(coordinate: end.coordinate, name: dayEnds.last?.name, distance: length))
        dayEnds = kept
        return dropped
    }

    // MARK: Cut

    /// The line cut at the day ends: one point list per day, in ride order. A day that starts at
    /// a gap starts at the next piece; a gap inside a day stays in it as a straight segment.
    public func dayLines() -> [[RoutePoint]] {
        guard line.count > 1 else { return [] }
        let measured = measuredLine
        var from = 0.0
        return dayEnds.map { end in
            defer { from = end.distance }
            return slice(measured, from: from, to: end.distance)
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
