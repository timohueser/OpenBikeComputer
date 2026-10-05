import Foundation

/// The transfers of a trip: a day end where the next day starts elsewhere.
extension Trip {
    /// A day end where the next day starts farther away than this is a transfer.
    public static let transferMinMeters = TripJoin.joinMeters

    /// Whether the next day starts more than ``transferMinMeters`` from where `day` ends.
    public func endsAtTransfer(_ day: Int) -> Bool {
        transferStart(after: day) != nil
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
    func transferStart(after day: Int) -> Int? {
        guard dayEnds.indices.contains(day) else { return nil }
        let vertices = measuredLine.vertices
        return pieceStarts.first { start in
            abs(vertices[start].distance - dayEnds[day].distance) < MeasuredLine.tieMeters
                && line[start - 1].coordinate.distance(to: line[start].coordinate) > Self.transferMinMeters
        }
    }
}
