import Foundation

/// The day editor's changes to a trip's day ends: split one line into days, even them out,
/// add, remove and move one. Every change keeps the invariants of ``Trip/reproject()``: day
/// ends stay in ride order, at least ``Trip/minimumDayMeters`` apart, and a day end at a
/// transfer stays put.
extension Trip {
    /// The most days the split stepper offers: a month of riding is the longest trip the
    /// device's route list is made for.
    public static let maxSplitDays = 30

    /// The most days a line of `length` splits into: every day at least twice
    /// ``minimumDayMeters``, so a re-projection never drops a day end, and never more than
    /// ``maxSplitDays``.
    public static func maxSplitDays(forLength length: Double) -> Int {
        max(1, min(maxSplitDays, Int(length / (2 * minimumDayMeters))))
    }

    /// The figures of every day, in ride order, stop routes included.
    public func dayStats() -> [DayStats] {
        dayStats(on: measuredLine, ends: dayEnds.dropLast().map(\.distance))
    }

    /// Cut the line into `days` days of equal riding time, each end snapped to a candidate
    /// stop. Every day is a new, unnamed day afterwards; the trip's name says what the line is.
    public mutating func split(into days: Int, candidates: [PlacedStop]) {
        guard let last = dayEnds.last, days >= 1 else { return }
        let measured = measuredLine
        let ends = DayBalance.ends(on: measured, bikeType: bikeType, days: days, candidates: candidates)
        dayEnds = ends.map { end in
            DayEnd(coordinate: measured.coordinate(at: end.distance), name: end.stop?.name, distance: end.distance, stop: end.stop)
        } + [DayEnd(coordinate: last.coordinate, name: last.name, distance: last.distance, transfer: last.transfer, resumeName: last.resumeName)]
    }

    /// Re-balance the day ends after `from` by riding time, keeping the number of days and each
    /// day's own name. A day end at a transfer stays, with its transfer, and the days on each
    /// side of it are balanced among themselves. `from` is where the balance starts: the line
    /// start, or the position a trip review re-plans from.
    public mutating func evenOut(from: Double = 0, candidates: [PlacedStop]) {
        let measured = measuredLine
        var stretchStart = from
        // An end within a tie of `from` is the one the balance starts after, not one to move.
        var first = dayEnds.firstIndex { $0.distance > from + MeasuredLine.tieMeters } ?? dayEnds.count
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

    /// A new day end where the rider put it, held ``minimumDayMeters`` inside the day it cuts.
    /// Returns the index of the new day, or nil when that day has no room for two.
    @discardableResult
    public mutating func addDayEnd(at distance: Double) -> Int? {
        guard let day = dayEnds.firstIndex(where: { $0.distance > distance }) else { return nil }
        let from = lineStart(of: day)
        let to = lineEnd(of: day)
        guard to - from >= 2 * Self.minimumDayMeters else { return nil }
        let held = min(max(distance, from + Self.minimumDayMeters), to - Self.minimumDayMeters)
        dayEnds.insert(DayEnd(coordinate: measuredLine.coordinate(at: held), distance: held), at: day)
        return day
    }

    /// The day whose end a stop can take: the nearest end along the line among those it may
    /// move. Nil when no day can end there. A caller that holds the measured line passes it.
    public func day(thatCanEndAt stop: PlacedStop, on measured: MeasuredLine? = nil) -> Int? {
        (0..<dayCount)
            .filter { endRange(of: $0, on: measured)?.contains(stop.distance) ?? false }
            .min { abs(dayEnds[$0].distance - stop.distance) < abs(dayEnds[$1].distance - stop.distance) }
    }

    /// Why `day`'s end cannot be removed, or nil when it can. The last day ends at the line end.
    /// A day end at a transfer holds a gap that must not move inside a day.
    public func removeDayEndBlocker(_ day: Int, on measured: MeasuredLine? = nil) -> String? {
        guard dayEnds.indices.contains(day), day < dayCount - 1 else { return "The last day ends the trip." }
        return endsAtTransfer(day, on: measured) ? "This day ends at a transfer." : nil
    }

    /// Take the days of an edited copy of this trip: the day editor's Done. The line comes too,
    /// for the gaps the editor bridged. Everything else, the device links above all, stays as
    /// it is now.
    public mutating func replaceDays(from edited: Trip) {
        line = edited.line
        pieceStarts = edited.pieceStarts
        dayEnds = edited.dayEnds
        reproject()
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
