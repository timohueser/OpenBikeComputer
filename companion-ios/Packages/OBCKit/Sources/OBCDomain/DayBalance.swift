import Foundation

/// One day's figures between two distances on a line, as the day editor shows them while a
/// handle moves.
public struct DayStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var climbMeters: Double
    public var duration: TimeInterval

    public init(distanceMeters: Double, climbMeters: Double, duration: TimeInterval) {
        self.distanceMeters = distanceMeters
        self.climbMeters = climbMeters
        self.duration = duration
    }
}

extension MeasuredLine {
    /// Seconds of riding from the start to `distance` on `bikeType`: the unit days are balanced
    /// in. The ETA table is linear in distance and climb, so a line without elevation costs by
    /// distance alone. O(log n), so a drag frame can afford it for every day.
    public func cost(to distance: Double, bikeType: BikeType) -> Double {
        let clamped = min(max(distance, 0), length)
        return bikeType.ridingTime(distanceMeters: clamped, ascentMeters: climb(from: 0, to: clamped))
    }

    /// The distance whose cost is `target`, by bisection to a tenth of a metre; the cost is
    /// monotone in the distance.
    public func distance(atCost target: Double, bikeType: BikeType) -> Double {
        var low = 0.0
        var high = length
        while high - low > 0.1 {
            let mid = (low + high) / 2
            if cost(to: mid, bikeType: bikeType) < target { low = mid } else { high = mid }
        }
        return high
    }

    /// The stats of the days cut at `ends`, the interior day ends in ascending order; the last
    /// day ends at the line end.
    public func dayStats(ends: [Double], bikeType: BikeType) -> [DayStats] {
        let bounds = [0] + ends + [length]
        return zip(bounds, bounds.dropFirst()).map { from, to in
            let distance = max(to - from, 0)
            let climb = climb(from: from, to: to)
            return DayStats(
                distanceMeters: distance, climbMeters: climb,
                duration: bikeType.ridingTime(distanceMeters: distance, ascentMeters: climb))
        }
    }
}

/// Balance and snap: the one algorithm behind the split stepper and the trip review's
/// re-balance. Days are equal in riding time, not in kilometres, so a day with a big
/// climb is shorter. Each ideal day end then snaps to a stop near it.
public enum DayBalance {
    /// A stop within this share of one day's riding time from the ideal day end is a candidate.
    public static let snapCostFraction = 0.15
    /// A stop farther off the line than this is no candidate.
    public static let snapOffsetMeters = 1_000.0
    /// No day is shorter than this share of the average day.
    public static let minimumDayFraction = 0.2

    /// One balanced day end: where it sits, and the stop it snapped to, if any.
    public struct End: Equatable, Sendable {
        public var distance: Double
        public var stop: Stop?

        public init(distance: Double, stop: Stop? = nil) {
            self.distance = distance
            self.stop = stop
        }
    }

    /// The `days - 1` interior day ends that cut `stretch` of `line` into `days` days of equal
    /// riding time, in ascending order. A candidate within ``snapCostFraction`` of an ideal end
    /// and at most ``snapOffsetMeters`` off the line wins it; of several, the one nearest along
    /// the line. Day ends never cross, and no day is shorter than ``minimumDayFraction`` of the
    /// average or than ``Trip/minimumDayMeters``: an end that would make one moves toward its
    /// ideal point until the day is long enough.
    public static func ends(
        on line: MeasuredLine, bikeType: BikeType, days: Int, candidates: [PlacedStop],
        stretch: ClosedRange<Double>? = nil
    ) -> [End] {
        let stretch = stretch ?? 0...line.length
        guard days > 1, stretch.upperBound > stretch.lowerBound else { return [] }
        func cost(_ distance: Double) -> Double { line.cost(to: distance, bikeType: bikeType) }
        func distance(atCost target: Double) -> Double {
            min(max(line.distance(atCost: target, bikeType: bikeType), stretch.lowerBound), stretch.upperBound)
        }
        let start = cost(stretch.lowerBound)
        let average = (cost(stretch.upperBound) - start) / Double(days)
        let usable = candidates.filter { $0.offset <= snapOffsetMeters && stretch.contains($0.distance) }

        var ends = (1..<days).map { day -> End in
            let target = start + average * Double(day)
            let ideal = distance(atCost: target)
            let snapped = usable
                .filter { abs(cost($0.distance) - target) <= snapCostFraction * average }
                .min { abs($0.distance - ideal) < abs($1.distance - ideal) }
            return End(distance: snapped?.distance ?? ideal, stop: snapped?.stop)
        }

        // The floor in riding time, and never under the trip's shortest day in metres.
        let floor = minimumDayFraction * average
        var previous = stretch.lowerBound
        for day in ends.indices {
            let lowest = max(distance(atCost: cost(previous) + floor), previous + Trip.minimumDayMeters)
            if ends[day].distance < lowest { ends[day] = End(distance: lowest) }
            previous = ends[day].distance
        }
        var next = stretch.upperBound
        for day in ends.indices.reversed() {
            let highest = min(distance(atCost: cost(next) - floor), next - Trip.minimumDayMeters)
            if ends[day].distance > highest { ends[day] = End(distance: highest) }
            next = ends[day].distance
        }
        return ends
    }
}
