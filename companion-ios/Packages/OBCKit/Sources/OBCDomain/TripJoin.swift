import Foundation

/// Joining several route files into one trip: the proposed ride order and the joins between
/// the files. The files become days in this order, with the day ends on the file boundaries.
public enum TripJoin {
    /// Two files closer than this at a boundary read as "joins"; farther apart, as a gap.
    public static let joinMeters = 200.0

    /// One file as the join reads it: its name and its end points.
    public struct File: Equatable, Sendable {
        public var name: String
        public var start: Coordinate
        public var end: Coordinate

        public init(name: String, start: Coordinate, end: Coordinate) {
            self.name = name
            self.start = start
            self.end = end
        }
    }

    /// The metres from the end of each file to the start of the next, in the order given.
    public static func gaps(_ files: [File]) -> [Double] {
        zip(files, files.dropFirst()).map { $0.end.distance(to: $1.start) }
    }

    /// A ride order for `files`, as indices. It chains the files by their end points: first the
    /// file whose start is far from every other end, then each time the file that starts nearest
    /// to where the last one ends. When that chain has no less gap than the natural filename
    /// order, the filename order wins, so loops and unrelated files keep the rider's names.
    public static func proposedOrder(_ files: [File]) -> [Int] {
        let natural = files.indices.sorted {
            files[$0].name.localizedStandardCompare(files[$1].name) == .orderedAscending
        }
        guard files.count > 1 else { return natural }
        func loneness(_ i: Int) -> Double {
            files.indices.filter { $0 != i }.map { files[i].start.distance(to: files[$0].end) }.min() ?? 0
        }
        var chain = [natural.max { loneness($0) < loneness($1) }!]
        var rest = natural.filter { $0 != chain[0] }
        while !rest.isEmpty {
            let end = files[chain[chain.count - 1]].end
            let next = rest.min { end.distance(to: files[$0].start) < end.distance(to: files[$1].start) }!
            chain.append(next)
            rest.removeAll { $0 == next }
        }
        func totalGap(_ order: [Int]) -> Double { gaps(order.map { files[$0] }).reduce(0, +) }
        return totalGap(chain) < totalGap(natural) ? chain : natural
    }
}
