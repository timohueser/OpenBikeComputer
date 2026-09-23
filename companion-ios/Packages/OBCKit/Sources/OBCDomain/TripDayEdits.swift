import Foundation

/// The day editor's changes to a trip's day ends: split one line into days, even them out,
/// add, remove and move one. Every change keeps the invariants of ``Trip/reproject()``: day
/// ends stay in ride order, at least ``Trip/minimumDayMeters`` apart, and a day end at a
/// transfer stays put.
extension Trip {
    /// The most days the split stepper offers: a month of riding is the longest trip the
    /// device's route list is made for.
    public static let maxSplitDays = 30

    /// The figures of every day, in ride order.
    public func dayStats() -> [DayStats] {
        measuredLine.dayStats(ends: dayEnds.dropLast().map(\.distance), bikeType: bikeType)
    }

    /// Cut the line into `days` days of equal riding time, each end snapped to a candidate
    /// stop. Every day is a new, unnamed day afterwards; the trip's name says what the line is.
    public mutating func split(into days: Int, candidates: [PlacedStop]) {
        guard let last = dayEnds.last, days >= 1 else { return }
        let measured = measuredLine
        let ends = DayBalance.ends(on: measured, bikeType: bikeType, days: days, candidates: candidates)
        dayEnds = ends.map { end in
            DayEnd(coordinate: measured.coordinate(at: end.distance), name: end.stop?.name, distance: end.distance, stop: end.stop)
        } + [DayEnd(coordinate: last.coordinate, name: last.name, distance: last.distance)]
    }

    /// Re-balance the day ends after `from` by riding time, keeping the number of days and each
    /// day's own name. A day end at a transfer stays, and the days on each side of it are
    /// balanced among themselves. `from` is where the balance starts: the line start, or the
    /// position a trip review re-plans from.
    public mutating func evenOut(from: Double = 0, candidates: [PlacedStop]) {
        let measured = measuredLine
        var stretchStart = from
        var first = dayEnds.firstIndex { $0.distance > from } ?? dayEnds.count
        while first < dayEnds.count - 1 {
            // The stretch runs up to the next fixed end: a transfer, or the line end.
            var fixed = first
            while fixed < dayEnds.count - 1, !endsAtTransfer(fixed) { fixed += 1 }
            let ends = DayBalance.ends(
                on: measured, bikeType: bikeType, days: fixed - first + 1, candidates: candidates,
                stretch: stretchStart...dayEnds[fixed].distance)
            for (day, end) in zip(first..<fixed, ends) {
                dayEnds[day] = DayEnd(
                    coordinate: measured.coordinate(at: end.distance), name: end.stop?.name,
                    title: dayEnds[day].title, distance: end.distance, stop: end.stop)
            }
            stretchStart = dayEnds[fixed].distance
            first = fixed + 1
        }
    }

    /// A new day end in the middle, by riding time, of the longest day. Returns the index of the
    /// new day, or nil when no day has room for two.
    @discardableResult
    public mutating func addDayEnd() -> Int? {
        let measured = measuredLine
        let stats = measured.dayStats(ends: dayEnds.dropLast().map(\.distance), bikeType: bikeType)
        guard let day = stats.indices.max(by: { stats[$0].duration < stats[$1].duration }) else { return nil }
        let from = day > 0 ? dayEnds[day - 1].distance : 0
        let to = dayEnds[day].distance
        guard to - from >= 2 * Self.minimumDayMeters else { return nil }
        let middle = (measured.cost(to: from, bikeType: bikeType) + measured.cost(to: to, bikeType: bikeType)) / 2
        let distance = min(
            max(measured.distance(atCost: middle, bikeType: bikeType), from + Self.minimumDayMeters),
            to - Self.minimumDayMeters)
        dayEnds.insert(DayEnd(coordinate: measured.coordinate(at: distance), distance: distance), at: day)
        return day
    }

    /// Why `day`'s end cannot be removed, or nil when it can. The last day ends at the line end.
    /// A day end at a transfer holds a gap that must not move inside a day.
    public func removeDayEndBlocker(_ day: Int) -> String? {
        guard dayEnds.indices.contains(day), day < dayCount - 1 else { return "The last day ends the trip." }
        return endsAtTransfer(day) ? "This day ends at a transfer." : nil
    }

    /// Remove `day`'s end, so the day joins the next one. False, and no change, when
    /// ``removeDayEndBlocker(_:)`` says why.
    @discardableResult
    public mutating func removeDayEnd(_ day: Int) -> Bool {
        guard removeDayEndBlocker(day) == nil else { return false }
        dayEnds.remove(at: day)
        return true
    }

    /// Move `day`'s end to `distance` along the line, held inside ``endRange(of:)``. The end
    /// leaves its stop and its place name behind; the day keeps its own name. False, and no
    /// change, for a day end that cannot move.
    @discardableResult
    public mutating func moveDayEnd(_ day: Int, to distance: Double) -> Bool {
        guard let range = endRange(of: day) else { return false }
        let held = min(max(distance, range.lowerBound), range.upperBound)
        guard held != dayEnds[day].distance else { return true }
        dayEnds[day] = DayEnd(coordinate: measuredLine.coordinate(at: held), title: dayEnds[day].title, distance: held)
        return true
    }
}
