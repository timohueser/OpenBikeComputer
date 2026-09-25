import Foundation

/// The rider's effort limits that were in force when a ride started
/// (`specs/obc-ble-interface-spec.md` §7.2). A ride keeps them when the rider's settings change
/// later. Nil is not set on the device at ride time, and that metric then has no zones.
public struct RideZoneLimits: Equatable, Sendable, Codable {
    public let maxHeartRate: Int?
    public let ftpWatts: Int?

    public init(maxHeartRate: Int?, ftpWatts: Int?) {
        self.maxHeartRate = maxHeartRate
        self.ftpWatts = ftpWatts
    }

    public static let notSet = RideZoneLimits(maxHeartRate: nil, ftpWatts: nil)

    /// The zone index `0...4` (Z1...Z5) of a heart rate, or nil without a max heart rate.
    public func heartRateZone(bpm: Int) -> Int? {
        maxHeartRate.map { Self.zone(bpm, limit: $0, edges: Self.heartRateEdges) }
    }

    /// The zone index `0...4` (Z1...Z5) of a power, or nil without an FTP.
    public func powerZone(watts: Int) -> Int? {
        ftpWatts.map { Self.zone(watts, limit: $0, edges: Self.powerEdges) }
    }

    /// The Z2...Z5 edges in percent of the limit, for zone bands behind a chart.
    public static var heartRateEdgePercents: [Int] { heartRateEdges.map(\.0) }
    public static var powerEdgePercents: [Int] { powerEdges.map(\.0) }

    /// The Z2...Z5 edges in percent of the limit, and whether a value exactly on the edge is
    /// already in the upper zone. The device's `effort.rs` holds the same table.
    private static let heartRateEdges = [(60, true), (70, true), (80, true), (90, true)]
    private static let powerEdges = [(55, true), (75, false), (90, false), (105, false)]

    private static func zone(_ value: Int, limit: Int, edges: [(Int, Bool)]) -> Int {
        let scaled = value * 100
        return edges.filter { edge, onEdgeIsUpper in
            onEdgeIsUpper ? scaled >= edge * limit : scaled > edge * limit
        }.count
    }
}
