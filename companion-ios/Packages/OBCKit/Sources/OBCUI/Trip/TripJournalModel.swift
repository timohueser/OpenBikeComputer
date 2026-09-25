import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The trip review, which reads as the journal of the trip: the planned line with the ridden
/// part, the totals, one entry per ridden day with its note and photos, and the offer to even
/// out the days after a day that ended far from its plan.
///
/// One per trip page. Each `load` replaces the last; a load that a newer one overtook changes
/// nothing. The page shows as soon as the rides are read, and the places the trip does not name
/// fill in when the geocoder answers.
@MainActor @Observable
public final class TripJournalModel {
    /// One ridden day.
    public struct Entry: Identifiable, Equatable {
        public let day: Int
        /// "Tue 30 Sep · Andermatt → Ulrichen · 74 km"; the places fill in when they are known.
        public var header: String
        public let note: String
        public let photos: [RidePhoto]
        public let thumbnails: [String: Data]
        public let rides: [RideSummary]

        public var id: Int { day }
    }

    /// The places on both sides of a transfer.
    public struct TransferPlaces: Equatable {
        public var from: String?
        public var to: String?
    }

    /// The trip of the last load.
    public private(set) var trip: Trip?
    /// Nil until loaded, and for a trip without a ride.
    public private(set) var review: TripReview?
    public private(set) var canReplay = false
    public private(set) var entries: [Entry] = []
    /// The line as the map draws it.
    public private(set) var runs: [LineRun] = []
    public private(set) var photoPins: [Coordinate] = []
    /// By the day the transfer follows.
    public private(set) var transfers: [Int: TransferPlaces] = [:]
    /// The re-balance offer until the rider uses or dismisses it.
    public private(set) var offer: RebalanceOffer?
    /// "Furka 2,431 m · Biggest day 82.0 km".
    public private(set) var highlights: String?

    /// About this many photo pins fit along a trip before they hide its line: a pin closer than
    /// the trip length over this to the last pin is left out.
    static let photoPinsPerTrip = 20

    @ObservationIgnored private let library: any LibraryStore
    @ObservationIgnored private var replayTracks: [RideID: [RidePoint]] = [:]
    @ObservationIgnored private let placeName: (@Sendable (Coordinate) async -> String?)?
    @ObservationIgnored private var generation = 0

    public init(library: any LibraryStore, placeName: (@Sendable (Coordinate) async -> String?)? = nil) {
        self.library = library
        self.placeName = placeName
    }

    /// "Days 3–4 are longer now. Even them out?"
    public var offerTitle: String? {
        offer.map { offer in
            "Days \(offer.days.lowerBound + 1)–\(offer.days.upperBound + 1) are \(offer.shortfall > 0 ? "longer" : "shorter") now. Even them out?"
        }
    }

    /// Build from the same edited tracks the review used, with each ride as a separate piece.
    public func replayContent() async -> ReplayContent? {
        guard let trip, let review, canReplay else { return nil }
        let title = trip.name
        let days: [(name: String, rides: [ReplayContent.Ride])] = review.days.enumerated().map { index, day in
            let rides = day.rides.map { ride in
                ReplayContent.Ride(
                    points: replayTracks[ride.id] ?? [],
                    photos: library.rideJournal(ride.id).photos,
                    thumbnails: library.ridePhotoThumbnails(ride.id)
                )
            }
            let name = trip.dayEnds[index].title.map { "Day \(index + 1) · \($0)" } ?? "Day \(index + 1)"
            return (name: name, rides: rides)
        }
        return await Task.detached(priority: .userInitiated) {
            ReplayContent.trip(title: title, days: days)
        }.value
    }

    /// Read `trip` against `rides`, the rides as the list shows them.
    public func load(trip: Trip, rides: [RideSummary]) async {
        generation += 1
        let current = generation
        let rides = rides.filter { $0.trip?.key == trip.key }
        var tracks: [RideID: [RidePoint]] = [:]
        for ride in rides { tracks[ride.id] = library.ridePoints(ride.id) ?? [] }
        let loaded = tracks
        let review = await Task.detached(priority: .userInitiated) {
            TripReview(trip: trip, rides: rides, tracks: loaded)
        }.value
        guard current == generation else { return }

        self.trip = trip
        self.review = review
        replayTracks = loaded
        canReplay = review?.days.flatMap(\.rides).contains {
            ReplayContent.hasUsableGeometry(loaded[$0.id] ?? [])
        } ?? false
        guard let review else {
            entries = []
            return
        }
        runs = trip.runs(ridden: review.ridden)
        var pins: [Coordinate] = []
        entries = review.days.indices.filter { !review.days[$0].rides.isEmpty }.map { index in
            let day = review.days[index]
            var photos: [RidePhoto] = []
            var thumbnails: [String: Data] = [:]
            for ride in day.rides {
                let journal = library.rideJournal(ride.id)
                photos += journal.photos
                thumbnails.merge(library.ridePhotoThumbnails(ride.id)) { first, _ in first }
                if let points = tracks[ride.id], !points.isEmpty {
                    pins += RidePhotoPlacement.place(journal.photos, on: points, line: MeasuredLine(ridePoints: points))
                        .map(\.coordinate)
                }
            }
            return Entry(
                day: index, header: Self.header(trip, index, day),
                note: library.dayNote(.tripDay(key: trip.key, dayIndex: index)),
                photos: photos, thumbnails: thumbnails, rides: day.rides)
        }
        let spacing = review.plannedMeters / Double(Self.photoPinsPerTrip)
        photoPins = pins.reduce(into: []) { kept, pin in
            if kept.last.map({ $0.distance(to: pin) >= spacing }) ?? true { kept.append(pin) }
        }
        transfers = Dictionary(uniqueKeysWithValues: trip.dayEnds.indices.filter { trip.endsAtTransfer($0) }.map {
            ($0, TransferPlaces(from: trip.dayEnds[$0].name, to: trip.dayStart($0 + 1)?.name))
        })
        offer = review.rebalance.flatMap { library.rideJournal($0.ride).closedRows.contains(.rebalance) ? nil : $0 }
        highlights = Self.highlights(review, trip: trip)
        await namePlaces(trip, review, tracks, generation: current)
    }

    /// The rider used or dismissed the offer: it never comes back for that day.
    public func closeOffer() {
        guard let offer else { return }
        var journal = library.rideJournal(offer.ride)
        journal.close(.rebalance)
        library.saveRideJournal(journal, thumbnails: [:], for: offer.ride)
        self.offer = nil
    }

    private static func header(
        _ trip: Trip, _ index: Int, _ day: TripReview.Day, from: String? = nil, to: String? = nil
    ) -> String {
        OBCFormat.dayNoteHeader(
            date: day.rides[0].date, from: from ?? trip.dayStart(index)?.name, to: to ?? trip.dayEnds[index].name,
            distanceMeters: day.totals.distanceMeters)
    }

    /// Ask the geocoder for the places the trip does not name: a day's start after a transfer, a
    /// day end the rider never named, and both sides of a transfer. A newer load stops it.
    private func namePlaces(
        _ trip: Trip, _ review: TripReview, _ tracks: [RideID: [RidePoint]], generation current: Int
    ) async {
        guard let placeName else { return }
        for entry in entries {
            let day = review.days[entry.day]
            let points = day.rides.flatMap { tracks[$0.id] ?? [] }
            var from = trip.dayStart(entry.day)?.name, to = trip.dayEnds[entry.day].name
            if from == nil, let first = points.first { from = await placeName(first.coordinate) }
            if to == nil, let last = points.last { to = await placeName(last.coordinate) }
            guard current == generation else { return }
            if let position = entries.firstIndex(where: { $0.day == entry.day }) {
                entries[position].header = Self.header(trip, entry.day, day, from: from, to: to)
            }
        }
        for day in transfers.keys.sorted() {
            var places = transfers[day] ?? TransferPlaces()
            if places.from == nil { places.from = await placeName(trip.dayEnds[day].coordinate) }
            if places.to == nil, let start = trip.dayStart(day + 1) { places.to = await placeName(start.coordinate) }
            guard current == generation else { return }
            transfers[day] = places
        }
    }

    private static func highlights(_ review: TripReview, trip: Trip) -> String? {
        var parts: [String] = []
        if let high = review.highPoint, let elevation = high.elevationMeters {
            let place = trip.waypoints.first {
                $0.coordinate.distance(to: high.coordinate) <= RideHighlights.placeRadiusMeters
            }?.name
            let height = "\(OBCFormat.climbValue(meters: elevation)) m"
            parts.append(place.map { "\($0) \(height)" } ?? "High point \(height)")
        }
        if let biggest = review.biggestDay {
            parts.append("Biggest day \(OBCFormat.distance(meters: review.days[biggest].totals.distanceMeters))")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

/// Place names by coordinate for the session, so opening a page again does not ask the
/// geocoder again. A lookup that finds nothing is asked again next time.
actor PlaceNameCache {
    private var names: [Coordinate: String] = [:]
    private let lookup: @Sendable (Coordinate) async -> String?

    init(lookup: @escaping @Sendable (Coordinate) async -> String?) {
        self.lookup = lookup
    }

    func name(at coordinate: Coordinate) async -> String? {
        if let name = names[coordinate] { return name }
        let name = await lookup(coordinate)
        if let name { names[coordinate] = name }
        return name
    }
}
