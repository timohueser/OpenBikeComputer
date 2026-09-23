import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The Tracked list's year and bike-type filter, its totals, and the all-rides map lines. The
/// list, the totals card and the map all show `filteredRides`.
@MainActor @Observable
public final class RideLibraryModel {
    /// The listed rides, newest first. The main screen sets it.
    public var rides: [RideSummary] = [] {
        didSet {
            if let bikeType, !bikeTypes.contains(bikeType) { self.bikeType = nil }
        }
    }
    public private(set) var bikeType: BikeType?
    /// Every listed ride's map line. Nil until `loadMapLines` finishes.
    public private(set) var mapLines: RideMapLines?

    /// The rider's year pick; `.some(nil)` is all years. Before a pick the newest ride's year
    /// shows, so a new year never opens on an empty list.
    private var pickedYear: Int??

    @ObservationIgnored private let library: any LibraryStore
    @ObservationIgnored private let calendar: Calendar
    @ObservationIgnored private let now: () -> Date

    public init(
        library: any LibraryStore,
        calendar: Calendar = .current,
        now: @escaping () -> Date = Date.init
    ) {
        self.library = library
        self.calendar = calendar
        self.now = now
    }

    public var year: Int? {
        pickedYear ?? rides.first.map { calendar.component(.year, from: $0.date) }
    }

    public var filter: RideFilter { RideFilter(year: year, bikeType: bikeType) }

    /// The year picker's choices, newest first; the current year is always there.
    public var years: [Int] { RideFilter.years(of: rides, now: now(), calendar: calendar) }

    /// The chips after All: each bike type with a ride in the year.
    public var bikeTypes: [BikeType] { RideTotals.byBikeType(ridesInYear).map(\.bikeType) }

    public var filteredRides: [RideSummary] {
        rides.filter { filter.includes($0, calendar: calendar) }
    }

    public var totals: RideTotals { RideTotals(filteredRides) }

    /// The map lines of `filteredRides`, or nil while they load.
    public var filteredMapLines: RideMapLines? {
        mapLines?.restricted(to: Set(filteredRides.map(\.id)))
    }

    /// A type without a ride in the new year has no chip, so its pick falls back to All.
    public func selectYear(_ year: Int?) {
        pickedYear = .some(year)
        if let bikeType, !bikeTypes.contains(bikeType) { self.bikeType = nil }
    }

    public func selectBikeType(_ type: BikeType?) {
        bikeType = type
    }

    /// Reads every listed ride's line off the main actor. The first read of a ride builds its
    /// line from the tracklog, one ride at a time.
    public func loadMapLines() async {
        let ids = rides.map(\.id)
        let library = library
        let lines = await Task.detached(priority: .userInitiated) {
            RideMapLines(ids.compactMap { library.rideMapLine($0) })
        }.value
        guard !Task.isCancelled else { return }
        mapLines = lines
    }

    private var ridesInYear: [RideSummary] {
        rides.filter { RideFilter(year: year).includes($0, calendar: calendar) }
    }
}
