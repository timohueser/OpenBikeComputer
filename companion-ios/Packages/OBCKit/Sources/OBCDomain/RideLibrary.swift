import Foundation

/// The ride library's scope: a calendar year and a bike type, each optional. The rides list, the
/// all-rides map and the totals apply the same filter.
public struct RideFilter: Equatable, Sendable {
    public var year: Int?
    public var bikeType: BikeType?

    public init(year: Int? = nil, bikeType: BikeType? = nil) {
        self.year = year
        self.bikeType = bikeType
    }

    /// A ride belongs to the year its start falls in, in `calendar`'s time zone.
    public func includes(_ ride: RideSummary, calendar: Calendar) -> Bool {
        if let bikeType, ride.bikeType != bikeType { return false }
        if let year, calendar.component(.year, from: ride.date) != year { return false }
        return true
    }

    /// The years the year picker offers: every year with a ride, and the current year, newest
    /// first.
    public static func years(
        of rides: some Sequence<RideSummary>, now: Date, calendar: Calendar
    ) -> [Int] {
        var years = Set(rides.map { calendar.component(.year, from: $0.date) })
        years.insert(calendar.component(.year, from: now))
        return years.sorted(by: >)
    }
}

/// Sums over rides. The input is the rides as the list shows them, so an edited ride counts once,
/// with its edited values and bike type.
public struct RideTotals: Equatable, Sendable {
    public var rideCount = 0
    public var distanceMeters = 0.0
    /// Seconds.
    public var movingTime: TimeInterval = 0
    public var climbMeters = 0.0

    public init(_ rides: some Sequence<RideSummary>) {
        for ride in rides {
            rideCount += 1
            distanceMeters += ride.distanceMeters
            movingTime += ride.movingTime
            climbMeters += ride.climbMeters
        }
    }

    /// The totals of each bike type, in `BikeType` order. A type without rides is absent, so its
    /// chip is hidden.
    public static func byBikeType(
        _ rides: some Sequence<RideSummary>
    ) -> [(bikeType: BikeType, totals: RideTotals)] {
        let groups = Dictionary(grouping: rides, by: \.bikeType)
        return BikeType.allCases.compactMap { type in
            groups[type].map { (type, RideTotals($0)) }
        }
    }
}
