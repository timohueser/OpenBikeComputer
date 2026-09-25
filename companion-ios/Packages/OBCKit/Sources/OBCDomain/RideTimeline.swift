import Foundation

/// A ride's samples on one distance axis: what the ride page's timeline plots and scrubs.
///
/// Zones, time in zones and the minimum, mean and maximum use every sample. A plot averages the
/// samples into drawing columns, so drawing and scrubbing cost the view's width, not the ride's
/// length.
public struct RideTimeline: Sendable {
    public enum Channel: Int, CaseIterable, Identifiable, Sendable {
        case elevation, speed, heartRate, power, cadence
        public var id: Int { rawValue }
    }

    /// Z1 to Z5.
    public static let zoneCount = 5

    /// The channels the ride has at least one sample for, in timeline order.
    public let channels: [Channel]
    public let limits: RideZoneLimits
    /// Distances run along this line, so a pause gap counts nothing.
    public let line: MeasuredLine
    public let maxGradePercent: Double?
    /// Seconds from the first fix, index-aligned with the line's vertices.
    private let seconds: [Double]
    /// Each channel's value at each fix; speed in metres per second.
    private let series: [Channel: [Double?]]

    /// Speed is measured over at least this many seconds on each side of a fix, because the
    /// distance between two 1 Hz fixes is mostly GPS noise.
    static let speedHalfWindow: TimeInterval = 5

    public init(ride: Ride) {
        let points = ride.points
        let line = MeasuredLine(ridePoints: points)
        let start = points.first?.timestamp ?? .distantPast
        let seconds = points.map { $0.timestamp.timeIntervalSince(start) }
        let series: [Channel: [Double?]] = [
            .elevation: points.map(\.elevationMeters),
            .speed: Self.speeds(line: line, seconds: seconds),
            .heartRate: points.map { $0.heartRate.map(Double.init) },
            .power: points.map { $0.power.map(Double.init) },
            .cadence: points.map { $0.cadence.map(Double.init) },
        ]
        self.line = line
        self.seconds = seconds
        self.series = series
        limits = ride.zoneLimits
        channels = Channel.allCases.filter { series[$0]?.contains { $0 != nil } == true }
        maxGradePercent = RouteStats.compute(
            from: points.map { RoutePoint(coordinate: $0.coordinate, elevationMeters: $0.elevationMeters) }
        ).maxGradePercent
    }

    public var length: Double { line.length }

    // MARK: Zones

    /// Whether the ride has a limit for this channel, so its values have zones.
    public func isZoned(_ channel: Channel) -> Bool {
        zoneEdges(channel) != nil
    }

    /// Index 0 is Z1. Nil for a channel without zones.
    public func zone(_ channel: Channel, value: Double) -> Int? {
        let rounded = Int(value.rounded())
        switch channel {
        case .heartRate: return limits.heartRateZone(bpm: rounded)
        case .power: return limits.powerZone(watts: rounded)
        case .elevation, .speed, .cadence: return nil
        }
    }

    /// The Z2...Z5 edges in the channel's unit, for zone bands behind a chart.
    public func zoneEdges(_ channel: Channel) -> [Double]? {
        let edges: (limit: Int?, percents: [Int])
        switch channel {
        case .heartRate: edges = (limits.maxHeartRate, RideZoneLimits.heartRateEdgePercents)
        case .power: edges = (limits.ftpWatts, RideZoneLimits.powerEdgePercents)
        case .elevation, .speed, .cadence: return nil
        }
        guard let limit = edges.limit, limit > 0 else { return nil }
        return edges.percents.map { Double($0 * limit) / 100 }
    }

    /// Seconds in each zone, Z1 first. A sample's zone holds from the fix before it; a pause gap
    /// and a sample without a value count nothing.
    public func timeInZones(_ channel: Channel) -> [TimeInterval]? {
        guard isZoned(channel), let values = series[channel] else { return nil }
        var totals = Array(repeating: 0.0, count: Self.zoneCount)
        for i in values.indices.dropFirst() where !line.pieceStarts.contains(i) {
            guard let value = values[i], let zone = zone(channel, value: value) else { continue }
            totals[zone] += seconds[i] - seconds[i - 1]
        }
        return totals
    }

    // MARK: Statistics

    public struct Stats: Equatable, Sendable {
        public let min: Double
        public let mean: Double
        public let max: Double
    }

    /// The mean weighs each sample by the time around it, so a fast stretch with dense fixes
    /// does not outweigh a slow one; it falls back to the plain mean without timing.
    public func stats(_ channel: Channel) -> Stats? {
        guard let values = series[channel] else { return nil }
        var (low, high) = (Double.infinity, -Double.infinity)
        var (sum, weights, plain, count) = (0.0, 0.0, 0.0, 0.0)
        for (i, value) in values.enumerated() {
            guard let value else { continue }
            low = min(low, value)
            high = max(high, value)
            let before = i > 0 && !line.pieceStarts.contains(i) ? seconds[i] - seconds[i - 1] : 0
            let after = i + 1 < values.count && !line.pieceStarts.contains(i + 1) ? seconds[i + 1] - seconds[i] : 0
            let weight = (before + after) / 2
            sum += value * weight
            weights += weight
            plain += value
            count += 1
        }
        guard count > 0 else { return nil }
        return Stats(min: low, mean: weights > 0 ? sum / weights : plain / count, max: high)
    }

    // MARK: Plotting

    /// One value per drawing column, nil where the channel has no sample: a pause gap or a
    /// sensor dropout.
    public struct Plot: Equatable, Sendable {
        public let values: [Double?]

        public static let empty = Plot(values: [])

        public var range: ClosedRange<Double>? {
            let known = values.compactMap { $0 }
            guard let low = known.min(), let high = known.max() else { return nil }
            return low...high
        }
    }

    /// Each column is the mean of its samples. A column between two adjacent fixes of one piece
    /// interpolates them, so a sparse track draws a line and not dots.
    public func plot(_ channel: Channel, columns: Int) -> Plot {
        guard columns > 0, length > 0, let values = series[channel] else { return .empty }
        var sums = Array(repeating: 0.0, count: columns)
        var counts = Array(repeating: 0, count: columns)
        var known: [(index: Int, distance: Double, value: Double)] = []
        for (i, value) in values.enumerated() {
            guard let value else { continue }
            let distance = line.vertices[i].distance
            let b = column(at: distance, columns: columns)
            sums[b] += value
            counts[b] += 1
            known.append((i, distance, value))
        }
        var plotted = [Double?](repeating: nil, count: columns)
        var next = 0
        for b in 0..<columns {
            if counts[b] > 0 {
                plotted[b] = sums[b] / Double(counts[b])
                continue
            }
            let x = distance(ofColumn: b, columns: columns)
            while next < known.count, known[next].distance <= x { next += 1 }
            guard next > 0, next < known.count else { continue }
            let (a, c) = (known[next - 1], known[next])
            guard c.index == a.index + 1, !line.pieceStarts.contains(c.index), c.distance > a.distance else { continue }
            plotted[b] = a.value + (c.value - a.value) * (x - a.distance) / (c.distance - a.distance)
        }
        return Plot(values: plotted)
    }

    /// The column that holds `distance`.
    public func column(at distance: Double, columns: Int) -> Int {
        guard length > 0 else { return 0 }
        return min(max(Int(distance / length * Double(columns)), 0), columns - 1)
    }

    /// The distance at a column's centre.
    public func distance(ofColumn column: Int, columns: Int) -> Double {
        length * (Double(column) + 0.5) / Double(columns)
    }

    /// The grade in percent at each column's centre, over `RouteStats.gradeWindowMeters`.
    public func grades(columns: Int) -> [Double] {
        guard line.hasElevation, length > 0, columns > 0 else { return [] }
        let half = RouteStats.gradeWindowMeters / 2
        return (0..<columns).map { b in
            let x = distance(ofColumn: b, columns: columns)
            let (from, to) = (max(0, x - half), min(length, x + half))
            guard to > from else { return 0 }
            return (line.elevation(at: to) - line.elevation(at: from)) / (to - from) * 100
        }
    }

    /// The device climb screen's grade band: 0 below 3 %, then 3–6, 6–9, 9–12, and 4 from 12 %.
    /// The grade truncates to whole percent first, as on the device.
    public static func gradeBand(percent: Double) -> Int {
        switch Int(percent) {
        case ..<3: return 0
        case 3..<6: return 1
        case 6..<9: return 2
        case 9..<12: return 3
        default: return 4
        }
    }

    // MARK: Building

    /// Distance over time across the fixes within `speedHalfWindow` on each side, and always
    /// at least one neighbour, never across a pause gap.
    private static func speeds(line: MeasuredLine, seconds: [Double]) -> [Double?] {
        let vertices = line.vertices
        var pieceStart = 0
        var pieceEnd = [Int](repeating: vertices.count - 1, count: vertices.count)
        var end = vertices.count - 1
        for i in vertices.indices.reversed() {
            pieceEnd[i] = end
            if line.pieceStarts.contains(i) { end = i - 1 }
        }
        return vertices.indices.map { i in
            if line.pieceStarts.contains(i) { pieceStart = i }
            var lo = i
            while lo > pieceStart, lo == i || seconds[i] - seconds[lo - 1] <= speedHalfWindow { lo -= 1 }
            var hi = i
            while hi < pieceEnd[i], hi == i || seconds[hi + 1] - seconds[i] <= speedHalfWindow { hi += 1 }
            let elapsed = seconds[hi] - seconds[lo]
            guard elapsed > 0 else { return nil }
            return (vertices[hi].distance - vertices[lo].distance) / elapsed
        }
    }
}
