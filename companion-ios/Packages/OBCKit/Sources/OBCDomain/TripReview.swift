import Foundation

/// A ride's track read against a trip line.
public struct LineCoverage: Equatable, Sendable {
    /// The parts of the line the track rode, in metres along the line: ascending and disjoint.
    public var ranges: [ClosedRange<Double>]
    /// Where the track was last on the line; nil when it never was.
    public var end: Double?

    public init(ranges: [ClosedRange<Double>] = [], end: Double? = nil) {
        self.ranges = ranges
        self.end = end
    }
}

extension Trip {
    /// Two on-line samples in a row join one ridden part when the line between them is at most
    /// this much longer than the step between them, which allows for a bend. A bigger jump is
    /// another leg of the line.
    public static let coverJoinMeters = 500.0
    /// A sample closer than this to the last one adds nothing. It bounds the work on a dense track.
    static let coverSampleMeters = 50.0

    /// A sample may fall this far behind the last match along the line, for GPS jitter at a stop.
    static let coverBackMeters = 50.0

    /// The parts of the line `track` rode. A sample at most ``onLineMeters`` from the line is on
    /// it, and a sample off the line ends a part. The first sample matches inside `day`, the
    /// day's planned range. Each next one matches in a window ahead of the last match, and a tie
    /// goes to the far side, so the return of an out-and-back stays on the return leg.
    public func coverage(of track: [Coordinate], day: ClosedRange<Double>) -> LineCoverage {
        guard line.count > 1 else { return LineCoverage() }
        let measured = measuredLine
        let ahead = Self.refineWindowMeters, back = Self.coverBackMeters
        var ranges: [ClosedRange<Double>] = []
        var last: Double?
        // The sample at `last`, while no sample off the line came after it.
        var onLine: Coordinate?
        var sampled: Coordinate?
        for (index, point) in track.enumerated() {
            if let sampled, index < track.count - 1, sampled.distance(to: point) < Self.coverSampleMeters { continue }
            sampled = point
            var projection: (distance: Double, error: Double)
            if let last {
                projection = measured.projection(of: point, near: last + (ahead - back) / 2, window: (ahead + back) / 2)
            } else {
                let half = (day.upperBound - day.lowerBound) / 2
                let coarse = measured.projection(of: point, near: day.lowerBound + half, window: half)
                projection = measured.projection(of: point, near: coarse.distance, window: Self.refineWindowMeters)
            }
            if projection.error > Self.onLineMeters {
                // Off the line near the last match: a detour, a pause that ended far away, or a
                // ride that starts outside its day.
                let coarse = measured.projection(of: point, near: last ?? day.lowerBound, window: measured.length)
                projection = measured.projection(of: point, near: coarse.distance, window: Self.refineWindowMeters)
            }
            guard projection.error <= Self.onLineMeters else {
                onLine = nil
                continue
            }
            let at = projection.distance
            if let previous = last, let from = onLine,
                abs(at - previous) <= from.distance(to: point) + Self.coverJoinMeters, let open = ranges.popLast() {
                ranges.append(min(open.lowerBound, at)...max(open.upperBound, at))
            } else {
                ranges.append(at...at)
            }
            last = at
            onLine = point
        }
        return LineCoverage(ranges: Self.merged(ranges), end: last)
    }

    static func merged(_ ranges: [ClosedRange<Double>]) -> [ClosedRange<Double>] {
        ranges.sorted { $0.lowerBound < $1.lowerBound }.reduce(into: []) { out, range in
            if let open = out.last, range.lowerBound <= open.upperBound {
                out[out.count - 1] = open.lowerBound...max(open.upperBound, range.upperBound)
            } else {
                out.append(range)
            }
        }
    }
}

/// A stretch of the trip line as the review map draws it.
public struct LineRun: Equatable, Sendable {
    public enum Kind: Sendable { case ridden, planned, transfer }
    public var kind: Kind
    public var coordinates: [Coordinate]
}

extension Trip {
    /// The line cut into ridden and planned runs at the bounds of `ridden`, with a straight
    /// transfer run across each transfer gap. A shorter gap draws nothing.
    public func runs(ridden: [ClosedRange<Double>]) -> [LineRun] {
        let measured = measuredLine
        let vertices = measured.vertices
        let bounds = ridden.flatMap { [$0.lowerBound, $0.upperBound] }
        var runs: [LineRun] = []
        func add(_ kind: LineRun.Kind, _ from: Coordinate, _ to: Coordinate) {
            if let last = runs.last, last.kind == kind, last.coordinates.last == from {
                runs[runs.count - 1].coordinates.append(to)
            } else {
                runs.append(LineRun(kind: kind, coordinates: [from, to]))
            }
        }
        for (a, b) in zip(vertices, vertices.dropFirst()) {
            if a.distance == b.distance {
                if a.coordinate.distance(to: b.coordinate) > Self.transferMinMeters { add(.transfer, a.coordinate, b.coordinate) }
                continue
            }
            func at(_ distance: Double) -> Coordinate {
                let t = (distance - a.distance) / (b.distance - a.distance)
                return Coordinate(
                    latitude: a.coordinate.latitude + (b.coordinate.latitude - a.coordinate.latitude) * t,
                    longitude: a.coordinate.longitude + (b.coordinate.longitude - a.coordinate.longitude) * t)
            }
            let cuts = bounds.filter { $0 > a.distance && $0 < b.distance }.sorted()
            var from = (distance: a.distance, coordinate: a.coordinate)
            for to in cuts.map({ ($0, at($0)) }) + [(b.distance, b.coordinate)] {
                let mid = (from.distance + to.0) / 2
                add(ridden.contains { $0.contains(mid) } ? .ridden : .planned, from.coordinate, to.1)
                from = to
            }
        }
        return runs
    }
}

/// The offer to even out the days after a day that ended far from its plan. It feeds the day
/// editor's Even out days: the days in ``days`` share the line after ``fixedBefore`` equally,
/// and nothing before it moves.
public struct RebalanceOffer: Equatable, Sendable {
    /// The ridden day that ended away from its planned end.
    public let day: Int
    /// The last ride of that day. Its journal remembers that the rider used or dismissed the offer.
    public let ride: RideID
    /// The unridden days to even out: the days after ``day`` up to the next transfer or the trip end.
    public let days: ClosedRange<Int>
    /// Where the day ended, in metres along the line.
    public let fixedBefore: Double
    /// The planned day end minus where the day ended. Positive when the day stopped early, so the
    /// days after it are longer.
    public let shortfall: Double
}

/// A trip read against its synced rides: each day planned and ridden, the ridden parts of the
/// line, the totals, and the re-balance offer. A ride belongs to the trip day it records, so the
/// grouping needs no dates.
public struct TripReview: Equatable, Sendable {
    public struct Day: Equatable, Sendable {
        /// Metres along the line from the day's start to its planned end. A transfer before the
        /// day counts nothing.
        public let planned: ClosedRange<Double>
        /// The day's rides in start order. A day with two rides counts both.
        public let rides: [RideSummary]
        /// Where the day's last ride was last on the line.
        public let endedAt: Double?

        public var plannedMeters: Double { planned.upperBound - planned.lowerBound }
        public var totals: RideTotals { RideTotals(rides) }
    }

    /// A day that ended more than this from its planned end offers to even out the days after it.
    public static let rebalanceMinMeters = 5_000.0

    public let days: [Day]
    /// The parts of the line the rides covered: ascending and disjoint.
    public let ridden: [ClosedRange<Double>]
    /// The length of the line. A transfer counts nothing.
    public let plannedMeters: Double
    /// The sums of the rides only, so a transfer adds nothing.
    public let totals: RideTotals
    /// The day after the last ridden day; nil once the last day is ridden.
    public let currentDay: Int?
    public let rebalance: RebalanceOffer?
    /// The highest ridden point with an elevation.
    public let highPoint: RidePoint?

    /// Nil before the trip has a ride: the trip page shows the plan. `rides` are the rides as the
    /// list shows them, so an edited ride counts once. `tracks` gives each ride's points; a ride
    /// without them counts in the totals but covers nothing.
    public init?(trip: Trip, rides: [RideSummary], tracks: [RideID: [RidePoint]]) {
        var byDay: [Int: [RideSummary]] = [:]
        for ride in rides {
            guard let day = ride.trip, day.key == trip.key, day.dayIndex < trip.dayCount else { continue }
            byDay[day.dayIndex, default: []].append(ride)
        }
        guard let lastRidden = byDay.keys.max() else { return nil }
        var coverage: [LineCoverage] = []
        var start = 0.0
        days = trip.dayEnds.enumerated().map { index, end in
            defer { start = end.distance }
            let dayRides = (byDay[index] ?? []).sorted { $0.date < $1.date }
            let covered = dayRides.map {
                trip.coverage(of: (tracks[$0.id] ?? []).map(\.coordinate), day: start...end.distance)
            }
            coverage += covered
            return Day(planned: start...end.distance, rides: dayRides, endedAt: covered.last?.end)
        }
        ridden = Trip.merged(coverage.flatMap(\.ranges))
        plannedMeters = trip.dayEnds.last?.distance ?? 0
        totals = RideTotals(days.flatMap(\.rides))
        currentDay = lastRidden + 1 < trip.dayCount ? lastRidden + 1 : nil
        rebalance = Self.rebalance(trip: trip, days: days, lastRidden: lastRidden)
        highPoint = days.flatMap(\.rides).flatMap { tracks[$0.id] ?? [] }
            .filter { $0.elevationMeters != nil }
            .max { ($0.elevationMeters ?? 0) < ($1.elevationMeters ?? 0) }
    }

    /// The ridden day with the most distance, once two days are ridden.
    public var biggestDay: Int? {
        let ridden = days.indices.filter { !days[$0].rides.isEmpty }
        guard ridden.count > 1 else { return nil }
        return ridden.max { days[$0].totals.distanceMeters < days[$1].totals.distanceMeters }
    }

    /// The offer after the last ridden day, when it ended more than ``rebalanceMinMeters`` from
    /// its plan and at least two unridden days can share the change. A day that ends at a
    /// transfer passes nothing on, because the next day starts elsewhere.
    private static func rebalance(trip: Trip, days: [Day], lastRidden day: Int) -> RebalanceOffer? {
        guard let ended = days[day].endedAt, let ride = days[day].rides.last, !trip.endsAtTransfer(day)
        else { return nil }
        let shortfall = days[day].planned.upperBound - ended
        guard abs(shortfall) > rebalanceMinMeters else { return nil }
        var last = day + 1
        while last < trip.dayCount - 1, !trip.endsAtTransfer(last) { last += 1 }
        guard last < trip.dayCount, last > day + 1 else { return nil }
        return RebalanceOffer(day: day, ride: ride.id, days: (day + 1)...last, fixedBefore: ended, shortfall: shortfall)
    }
}
