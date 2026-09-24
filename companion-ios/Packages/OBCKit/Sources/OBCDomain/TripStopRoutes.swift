import Foundation

/// Days that end at a stop off the line: the out and back and the via, routed on the phone, and
/// what they add to the days they touch.
extension Trip {
    /// A stop route stays through a line change while its junctions lie this close to the new
    /// line; farther, the day ends on the line again.
    public static let stopRouteKeepMeters = 50.0
    /// A via leaves the line this far before the stop's line point and rejoins this far after it:
    /// a short detour, well inside the router's reach. Each end stays in the half of its
    /// neighbouring day nearest the stop and on the stop's piece of the line.
    public static let viaReachMeters = 2_000.0

    /// Where `day` leaves the line: its end, or where its via leaves.
    public func lineEnd(of day: Int) -> Double {
        if case .via(_, _, let leave, _)? = dayEnds[day].stopRoute { return leave }
        return dayEnds[day].distance
    }

    /// Where `day` rides onto the line: the line start, the day end before it, or where the via
    /// of the day before rejoins.
    public func lineStart(of day: Int) -> Double {
        guard day > 0 else { return 0 }
        if case .via(_, _, _, let rejoin)? = dayEnds[day - 1].stopRoute { return rejoin }
        return dayEnds[day - 1].distance
    }

    /// Where a via around `day`'s end leaves and rejoins the line. Nil for the last day, a day
    /// end without a stop, or one with no room on either side.
    public func viaEnds(of day: Int) -> (leave: Double, rejoin: Double)? {
        guard day >= 0, day < dayCount - 1, dayEnds[day].stop != nil else { return nil }
        let vertices = measuredLine.vertices
        let junction = dayEnds[day].distance
        let starts = pieceStarts.map { vertices[$0].distance }
        let pieceFrom = starts.last { $0 <= junction } ?? 0
        let pieceTo = starts.first { $0 > junction } ?? (vertices.last?.distance ?? 0)
        let leave = max(junction - Self.viaReachMeters, (lineStart(of: day) + junction) / 2, pieceFrom)
        let rejoin = min(junction + Self.viaReachMeters, (junction + lineEnd(of: day + 1)) / 2, pieceTo)
        guard leave < junction - MeasuredLine.tieMeters, rejoin > junction + MeasuredLine.tieMeters else { return nil }
        return (leave, rejoin)
    }

    /// Route an out and back from `day`'s end to its stop.
    public func routeOutAndBack(
        _ day: Int, with router: any LegRouter, onDownload: @escaping @Sendable () -> Void = {}
    ) async throws -> StopRoute {
        guard dayEnds.indices.contains(day), let stop = dayEnds[day].stop else { throw LegRouteFailure.noRoad }
        let spur = try await router.route(
            from: dayEnds[day].coordinate, to: stop.coordinate, bikeType: bikeType, onDownload: onDownload)
        guard spur.count > 1 else { throw LegRouteFailure.noRoad }
        return .outAndBack(spur: [measuredLine.point(at: dayEnds[day].distance)] + spur)
    }

    /// Route a via through `day`'s stop: one leg from the line before the stop to the stop, one
    /// from the stop to the line after it.
    public func routeVia(
        _ day: Int, with router: any LegRouter, onDownload: @escaping @Sendable () -> Void = {}
    ) async throws -> StopRoute {
        guard let ends = viaEnds(of: day), let stop = dayEnds[day].stop else { throw LegRouteFailure.noRoad }
        let measured = measuredLine
        async let toStop = router.route(
            from: measured.coordinate(at: ends.leave), to: stop.coordinate, bikeType: bikeType, onDownload: onDownload)
        async let fromStop = router.route(
            from: stop.coordinate, to: measured.coordinate(at: ends.rejoin), bikeType: bikeType, onDownload: onDownload)
        let legs = try await (toStop, fromStop)
        guard legs.0.count > 1, legs.1.count > 1 else { throw LegRouteFailure.noRoad }
        return .via(
            toStop: [measured.point(at: ends.leave)] + legs.0, fromStop: legs.1 + [measured.point(at: ends.rejoin)],
            leave: ends.leave, rejoin: ends.rejoin)
    }

    /// Reach `day`'s stop by `route`, or end the day on the line with nil. False, and no change,
    /// for a day end without a stop, or a via whose ends lie outside the days around the stop.
    @discardableResult
    public mutating func setStopRoute(_ day: Int, _ route: StopRoute?) -> Bool {
        guard day >= 0, day < dayCount - 1, dayEnds[day].stop != nil else { return false }
        if case .via(_, _, let leave, let rejoin)? = route {
            let junction = dayEnds[day].distance
            guard leave >= lineStart(of: day), leave < junction, rejoin > junction, rejoin <= lineEnd(of: day + 1)
            else { return false }
        }
        dayEnds[day].stopRoute = route
        return true
    }

    /// The metres `route` adds to the trip at `day`'s end.
    public func extraMeters(_ route: StopRoute, at day: Int) -> Double {
        let change = route.change(on: measuredLine, junction: dayEnds[day].distance)
        return change.end.distance + change.start.distance
    }

    /// The figures of every day as its route rides, for day ends at `ends` (the trip's own, or a
    /// drag's in flight): the line between them, changed by the stop routes of the trip's day
    /// ends. `line` is ``measuredLine``, passed so a drag frame does not measure it again.
    public func dayStats(on line: MeasuredLine, ends: [Double]) -> [DayStats] {
        var stats = line.dayStats(ends: ends, bikeType: bikeType)
        for (day, end) in dayEnds.enumerated() where day + 1 < stats.count {
            guard let route = end.stopRoute else { continue }
            let change = route.change(on: line, junction: end.distance)
            stats[day].add(change.end, bikeType: bikeType)
            stats[day + 1].add(change.start, bikeType: bikeType)
        }
        return stats
    }
}

extension StopRoute {
    /// What the route changes in the day that ends at the stop and in the next day, against the
    /// line to and from `junction`.
    func change(on line: MeasuredLine, junction: Double) -> (end: StopRouteChange, start: StopRouteChange) {
        switch self {
        case .outAndBack(let spur):
            let out = MeasuredLine(routePoints: spur)
            return (
                StopRouteChange(distance: out.length, climb: out.climb(from: 0, to: out.length)),
                StopRouteChange(distance: out.length, climb: out.descent(from: 0, to: out.length)))
        case .via(let toStop, let fromStop, let leave, let rejoin):
            let to = MeasuredLine(routePoints: toStop)
            let from = MeasuredLine(routePoints: fromStop)
            return (
                StopRouteChange(
                    distance: to.length - (junction - leave),
                    climb: to.climb(from: 0, to: to.length) - line.climb(from: leave, to: junction)),
                StopRouteChange(
                    distance: from.length - (rejoin - junction),
                    climb: from.climb(from: 0, to: from.length) - line.climb(from: junction, to: rejoin)))
        }
    }

    /// The route of the reversed trip, on a line `length` metres long. A spur stays as it is; a
    /// via's legs swap and turn around.
    func reversed(length: Double) -> StopRoute {
        switch self {
        case .outAndBack: self
        case .via(let toStop, let fromStop, let leave, let rejoin):
            .via(toStop: fromStop.reversed(), fromStop: toStop.reversed(), leave: length - rejoin, rejoin: length - leave)
        }
    }

    /// The route on a changed line, where the day end moved from `old` to `new` metres along it
    /// and lies `error` metres from it. Nil when a junction lies farther than
    /// ``Trip/stopRouteKeepMeters`` from the line.
    func reprojected(on line: MeasuredLine, old: Double, new: Double, error: Double) -> StopRoute? {
        let keep = Trip.stopRouteKeepMeters
        switch self {
        case .outAndBack:
            return error <= keep ? self : nil
        case .via(let toStop, let fromStop, let leave, let rejoin):
            guard let out = toStop.first, let back = fromStop.last else { return nil }
            let window = 2 * Trip.viaReachMeters
            let newLeave = line.projection(of: out.coordinate, near: new - (old - leave), window: window)
            let newRejoin = line.projection(of: back.coordinate, near: new + (rejoin - old), window: window)
            guard newLeave.error <= keep, newRejoin.error <= keep, newLeave.distance < new, newRejoin.distance > new
            else { return nil }
            return .via(toStop: toStop, fromStop: fromStop, leave: newLeave.distance, rejoin: newRejoin.distance)
        }
    }
}

/// What a stop route adds to one day: metres and climb, each possibly negative for a via.
struct StopRouteChange: Equatable {
    var distance: Double
    var climb: Double
}

extension DayStats {
    mutating func add(_ change: StopRouteChange, bikeType: BikeType) {
        distanceMeters = max(0, distanceMeters + change.distance)
        climbMeters = max(0, climbMeters + change.climb)
        duration = bikeType.ridingTime(distanceMeters: distanceMeters, ascentMeters: climbMeters)
    }
}

extension MeasuredLine {
    /// The point `distance` metres along the line, elevation included.
    func point(at distance: Double) -> RoutePoint {
        RoutePoint(coordinate: coordinate(at: distance), elevationMeters: hasElevation ? elevation(at: distance) : nil)
    }
}
