import Foundation

/// Stops measured against the trip line, and a day that ends at a stop.
extension Trip {
    /// A stop at most this far from the line is on the line.
    public static let onLineMeters = 150.0
    /// The part of the line a stop found near a point projects onto: this far before and after
    /// the point. It keeps a stop on its own leg of an out-and-back.
    public static let stopWindowMeters = 10_000.0

    /// `stops` measured against the line, in the order given. With `near`, each stop
    /// projects onto the line within `window` of that distance; without it, onto the nearest
    /// point of the whole line.
    public func place(_ stops: [Stop], near: Double? = nil, window: Double = stopWindowMeters) -> [PlacedStop] {
        guard line.count > 1 else { return [] }
        let measured = measuredLine
        return stops.map { stop in
            let coarse = near.map {
                measured.projection(of: stop.coordinate, near: $0, window: window)
            } ?? measured.projection(of: stop.coordinate, near: 0, window: measured.length)
            let fine = measured.projection(of: stop.coordinate, near: coarse.distance, window: Self.refineWindowMeters)
            return PlacedStop(stop: stop, distance: fine.distance, offset: fine.error)
        }
    }

    /// A day end where the next day starts farther away than this is a transfer.
    public static let transferMinMeters = TripJoin.joinMeters

    /// The straight metres from where `day` ends to where the next day starts, when that is more
    /// than ``transferMinMeters``; else nil. A caller that holds the measured line passes it.
    public func transferMeters(after day: Int, on measured: MeasuredLine? = nil) -> Double? {
        transferStart(after: day, on: measured).map { line[$0 - 1].coordinate.distance(to: line[$0].coordinate) }
    }

    /// Whether the next day starts more than ``transferMinMeters`` from where `day` ends.
    /// Moving such a day end would put the gap inside a day. A caller that holds the measured
    /// line passes it, so the answer costs no line walk.
    public func endsAtTransfer(_ day: Int, on measured: MeasuredLine? = nil) -> Bool {
        transferStart(after: day, on: measured) != nil
    }

    /// Label the transfer after `day`. Nil clears it. No change where the day does not end at a
    /// transfer.
    public mutating func setTransfer(_ day: Int, to kind: TransferKind?) {
        guard endsAtTransfer(day) else { return }
        dayEnds[day].transfer = kind
    }

    /// Where `day` starts: the line start, the next piece after a transfer, or the day end before
    /// it, with the place name the trip keeps for it.
    public func dayStart(_ day: Int) -> (coordinate: Coordinate, name: String?)? {
        guard dayEnds.indices.contains(day) else { return nil }
        guard day > 0 else { return line.first.map { ($0.coordinate, startName) } }
        if let start = transferStart(after: day - 1) { return (line[start].coordinate, dayEnds[day - 1].resumeName) }
        return (dayEnds[day - 1].coordinate, dayEnds[day - 1].name)
    }

    /// The line index where the next day starts after a transfer at the end of `day`.
    func transferStart(after day: Int, on measured: MeasuredLine? = nil) -> Int? {
        guard dayEnds.indices.contains(day) else { return nil }
        let vertices = (measured ?? measuredLine).vertices
        return pieceStarts.first { start in
            abs(vertices[start].distance - dayEnds[day].distance) < MeasuredLine.tieMeters
                && line[start - 1].coordinate.distance(to: line[start].coordinate) > Self.transferMinMeters
        }
    }

    /// The distances where `day` can end: after the day before it and before the day after it,
    /// each by at least ``minimumDayMeters`` and clear of their vias. Nil for the last day, which
    /// ends at the line end, and for a day that ends at a transfer. A caller that holds the
    /// measured line passes it.
    public func endRange(of day: Int, on measured: MeasuredLine? = nil) -> ClosedRange<Double>? {
        guard day >= 0, day < dayCount - 1, !endsAtTransfer(day, on: measured) else { return nil }
        let lower = lineStart(of: day) + Self.minimumDayMeters
        let upper = lineEnd(of: day + 1) - Self.minimumDayMeters
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
