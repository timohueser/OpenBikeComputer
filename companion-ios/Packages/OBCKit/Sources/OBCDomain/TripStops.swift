import Foundation

/// Stops measured against the trip line, and a day that ends at a stop.
extension Trip {
    /// A stop at most this far from the line is on the line.
    public static let onLineMeters = 150.0
    /// The part of the line a stop found near a point projects onto: this far before and after
    /// the point. It keeps a stop on its own leg of an out-and-back.
    public static let stopWindowMeters = 10_000.0

    /// `stops` measured against the line, in order along the line. With `near`, each stop
    /// projects onto the line within ``stopWindowMeters`` of that distance; without it, onto the
    /// nearest point of the whole line.
    public func place(_ stops: [Stop], near: Double? = nil) -> [PlacedStop] {
        guard line.count > 1 else { return [] }
        let measured = measuredLine
        return stops.map { stop in
            let coarse = near.map {
                measured.projection(of: stop.coordinate, near: $0, window: Self.stopWindowMeters)
            } ?? measured.projection(of: stop.coordinate, near: 0, window: measured.length)
            let fine = measured.projection(of: stop.coordinate, near: coarse.distance, window: Self.refineWindowMeters)
            return PlacedStop(stop: stop, distance: fine.distance, offset: fine.error)
        }
        .sorted { $0.distance < $1.distance }
    }

    /// The distances where `day` can end: after the day before it and before the day after it,
    /// each by at least ``minimumDayMeters``. Nil for the last day, which ends at the line end.
    public func endRange(of day: Int) -> ClosedRange<Double>? {
        guard day >= 0, day < dayCount - 1 else { return nil }
        let lower = (day > 0 ? dayEnds[day - 1].distance : 0) + Self.minimumDayMeters
        let upper = dayEnds[day + 1].distance - Self.minimumDayMeters
        return lower <= upper ? lower...upper : nil
    }

    /// End `day` at `stop`: the day end moves to the line point nearest the stop and takes the
    /// stop's name. False, and no change, when the stop lies outside ``endRange(of:)``.
    @discardableResult
    public mutating func endDay(_ day: Int, at stop: PlacedStop) -> Bool {
        guard let range = endRange(of: day), range.contains(stop.distance) else { return false }
        dayEnds[day] = DayEnd(
            coordinate: measuredLine.coordinate(at: stop.distance), name: stop.stop.name,
            title: dayEnds[day].title, distance: stop.distance, stop: stop.stop)
        return true
    }
}

extension DayEnd {
    /// Metres from the day end to its stop. Nil without a stop.
    public var stopOffset: Double? { stop.map { $0.coordinate.distance(to: coordinate) } }
}
