import Foundation

/// One time range of one synced ride. Both ends are inclusive.
public struct RideSlice: Hashable, Sendable {
    public var source: RideID
    public var start: Date
    public var end: Date

    public init(source: RideID, start: Date, end: Date) {
        self.source = source
        self.start = start
        self.end = end
    }
}

/// An edited ride: slices of synced rides, in time order. The synced rides never change, so an
/// edit can always be reverted. An unedited ride has no view and shows as it was synced.
public struct RideView: Identifiable, Equatable, Sendable {
    /// What the lists show: the stats of the slices' points, and the rider's name and bike type.
    public var summary: RideSummary
    public var slices: [RideSlice]

    public var id: RideID { summary.id }
    public var sources: Set<RideID> { Set(slices.map(\.source)) }

    public init(summary: RideSummary, slices: [RideSlice]) {
        self.summary = summary
        self.slices = slices
    }
}

/// The pure half of ride editing: slice arithmetic and the points a list of slices stands for.
public enum RideEdit {
    /// The slices' points in order. The first point of each later slice starts a segment, so the
    /// time and the distance between two joined rides do not count. Nil when a source does not load.
    public static func points(
        of slices: [RideSlice], source: (RideID) -> [RidePoint]?
    ) -> [RidePoint]? {
        var points: [RidePoint] = []
        for slice in slices {
            guard let all = source(slice.source) else { return nil }
            var part = all.filter { $0.timestamp >= slice.start && $0.timestamp <= slice.end }
            if !points.isEmpty, !part.isEmpty { part[0].segmentStart = true }
            points += part
        }
        return points
    }

    /// The slices clipped to `range`; a slice outside it drops out.
    public static func trimmed(_ slices: [RideSlice], to range: ClosedRange<Date>) -> [RideSlice] {
        slices.compactMap { slice in
            let start = max(slice.start, range.lowerBound), end = min(slice.end, range.upperBound)
            return start <= end ? RideSlice(source: slice.source, start: start, end: end) : nil
        }
    }

    /// The slices before and after `time`. The point at `time` ends the first part and starts
    /// the second, so the two parts meet without a gap.
    public static func split(
        _ slices: [RideSlice], at time: Date
    ) -> (before: [RideSlice], after: [RideSlice]) {
        (trimmed(slices, to: .distantPast...time), trimmed(slices, to: time...Date.distantFuture))
    }

    /// `first` then `second`. Where both sides of a split meet again, the two slices become one,
    /// so a split and a merge give back the ride without a gap.
    public static func joined(_ first: [RideSlice], _ second: [RideSlice]) -> [RideSlice] {
        guard var last = first.last, let next = second.first,
              last.source == next.source, last.end >= next.start
        else { return first + second }
        last.end = max(last.end, next.end)
        return first.dropLast() + [last] + second.dropFirst()
    }

    /// "‹name› (n)" with the smallest n from `number` whose name no ride has, ignoring case as the
    /// import's name collision does.
    public static func freeName(_ name: String, from number: Int, taken: some Sequence<String>) -> String {
        let taken = Set(taken.map { $0.lowercased() })
        return (number...).lazy.map { "\(name) (\($0))" }.first { !taken.contains($0.lowercased()) }!
    }

    /// `views` without the views that share a source with the view `id`, and without the views
    /// that share a source with those, and so on. Each freed source shows again as it was synced.
    public static func reverted(_ views: [RideView], id: RideID) -> [RideView] {
        guard let view = views.first(where: { $0.id == id }) else { return views }
        var freed = view.sources
        var kept = views
        while let index = kept.firstIndex(where: { !$0.sources.isDisjoint(with: freed) }) {
            freed.formUnion(kept.remove(at: index).sources)
        }
        return kept
    }

    /// Whether two rides, `first` then `second`, look like one ride with a break: the second
    /// starts within `maxGapMeters` and `maxGapSeconds` of where the first ended, on the same
    /// trip day or both without a trip.
    public static func suggestsMerge(_ first: Ride, _ second: Ride) -> Bool {
        guard let end = first.points.last, let start = second.points.first else { return false }
        let gap = start.timestamp.timeIntervalSince(end.timestamp)
        guard gap >= 0, gap <= maxGapSeconds,
              end.coordinate.routeDistance(to: start.coordinate) <= maxGapMeters
        else { return false }
        switch (first.summary.trip, second.summary.trip) {
        case (nil, nil): return true
        case let (a?, b?): return a.key == b.key && a.dayIndex == b.dayIndex
        default: return false
        }
    }

    public static let maxGapMeters = 500.0
    public static let maxGapSeconds: TimeInterval = 12 * 3600
}

/// A merge the rider dismissed: `first` then `second`.
public struct RidePair: Hashable, Sendable {
    public var first: RideID
    public var second: RideID

    public init(first: RideID, second: RideID) {
        self.first = first
        self.second = second
    }
}

extension RideSummary {
    /// This summary with the stats of `points`, counted by the device's rules
    /// (`firmware/obc-app/src/recorder.rs`). Every interval inside a segment counts: the device
    /// starts a new segment after each interval it does not count. Moving time, the average speed
    /// and the sensor averages count only intervals at 0.8 m/s or faster. The climb has the
    /// device's 3 m dead band and restarts at each segment. The name, the bike type and the trip
    /// stay; a view is no device object, so it has no source.
    public func withStats(of points: [RidePoint], id: RideID) -> RideSummary {
        var ridden = 0.0, movingMeters = 0.0, moving = 0.0, climb = 0.0
        var confirmed: Double?
        var heartRate = Weighted(), cadence = Weighted(), power = Weighted()
        for (index, point) in points.enumerated() {
            if point.segmentStart { confirmed = nil }
            if let elevation = point.elevationMeters {
                if let last = confirmed {
                    if elevation >= last + RouteStats.climbHysteresisMeters {
                        climb += elevation - last
                        confirmed = elevation
                    } else if elevation <= last - RouteStats.climbHysteresisMeters {
                        confirmed = elevation
                    }
                } else {
                    confirmed = elevation
                }
            }
            guard index > 0, !point.segmentStart else { continue }
            let previous = points[index - 1]
            let dt = point.timestamp.timeIntervalSince(previous.timestamp)
            let distance = previous.coordinate.routeDistance(to: point.coordinate)
            guard dt > 0 else { continue }
            ridden += distance
            guard distance / dt >= 0.8 else { continue }
            movingMeters += distance
            moving += dt
            heartRate.add(point.heartRate, dt)
            cadence.add(point.cadence, dt)
            power.add(point.power, dt)
        }
        return RideSummary(
            id: id, name: name, date: points.first?.timestamp ?? date, distanceMeters: ridden,
            movingTime: moving, averageSpeedMps: moving > 0 ? movingMeters / moving : 0,
            climbMeters: climb, trackPreview: TrackPreview.normalizing(points.map(\.coordinate)),
            avgHeartRate: heartRate.average, maxHeartRate: heartRate.max, avgCadence: cadence.average,
            avgPower: power.average, maxPower: power.max, bikeType: bikeType, trip: trip
        )
    }
}

/// A time-weighted sensor average.
private struct Weighted {
    private var sum = 0.0
    private var seconds = 0.0
    private(set) var max: Int?

    mutating func add(_ value: Int?, _ dt: Double) {
        guard let value else { return }
        sum += Double(value) * dt
        seconds += dt
        max = Swift.max(max ?? value, value)
    }

    var average: Int? { seconds > 0 ? Int((sum / seconds).rounded()) : nil }
}
