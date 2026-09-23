import Foundation
import Observation
import OBCDomain

/// The stops of one day end: the campsites and hotels near it, the trip's waypoints near it, and a
/// search for any other place. Picking a stop ends the day there.
@MainActor @Observable
public final class TripStopsModel: Identifiable {
    public enum Nearby: Equatable, Sendable {
        case loading
        case loaded
        /// Apple Maps did not answer. The waypoints still show.
        case offline
    }

    public let day: Int
    public private(set) var nearby: Nearby = .loading
    /// The stops near the day end, in order along the line.
    public private(set) var stops: [PlacedStop] = []
    public var query = ""
    /// The places the last search found, in Apple Maps' order. Nil before a search.
    public private(set) var results: [PlacedStop]?

    @ObservationIgnored private let trip: Trip
    @ObservationIgnored private let finder: StopFinder?
    @ObservationIgnored private let isOnline: Bool
    @ObservationIgnored private let onPick: (PlacedStop) -> Void

    /// `finder` nil reads as offline.
    public init(
        trip: Trip, day: Int, finder: StopFinder?, isOnline: Bool, onPick: @escaping (PlacedStop) -> Void
    ) {
        self.trip = trip
        self.day = day
        self.finder = finder
        self.isOnline = isOnline
        self.onPick = onPick
    }

    private var dayEnd: DayEnd { trip.dayEnds[day] }

    /// The day end's distance along the line, which the rows measure "before" and "after" from.
    public var dayEndDistance: Double { dayEnd.distance }

    public var canSearch: Bool { isOnline && finder != nil }

    /// Show the waypoints at once, then add what Apple Maps finds.
    public func load() async {
        let waypoints = trip.waypoints.filter {
            $0.coordinate.distance(to: dayEnd.coordinate) <= StopFinder.radiusMeters
        }
        stops = alongTheLine(waypoints)
        guard isOnline, let finder else {
            nearby = .offline
            return
        }
        do {
            let found = try await finder.stops(near: dayEnd.distance, on: trip.measuredLine)
            stops = alongTheLine(found + waypoints)
            nearby = .loaded
        } catch {
            nearby = .offline
        }
    }

    private func alongTheLine(_ stops: [Stop]) -> [PlacedStop] {
        trip.place(stops, near: dayEnd.distance).sorted { $0.distance < $1.distance }
    }

    /// Search for `query` in the trip's region. A failed search finds nothing.
    public func search() async {
        let text = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, canSearch, let finder else { return }
        let places = (try? await finder.places(matching: text, along: trip.measuredLine)) ?? []
        results = trip.place(places)
    }

    /// Whether the day can end at `stop`: it must stay after the day before and before the day
    /// after.
    public func canPick(_ stop: PlacedStop) -> Bool {
        trip.endRange(of: day)?.contains(stop.distance) ?? false
    }

    public func pick(_ stop: PlacedStop) {
        guard canPick(stop) else { return }
        onPick(stop)
    }
}
