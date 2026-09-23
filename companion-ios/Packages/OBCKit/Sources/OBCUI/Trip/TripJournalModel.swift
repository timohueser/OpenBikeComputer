import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The trip review, which reads as the journal of the trip: the planned line with the ridden
/// part, the totals, one entry per ridden day with its note and photos, and the offer to even
/// out the days after a day that ended far from its plan.
///
/// Built for one trip and one ride list; the trip page builds a new one when either changes.
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

    /// Nil until loaded, and for a trip without a ride.
    public private(set) var review: TripReview?
    public private(set) var entries: [Entry] = []
    /// The line as the map draws it.
    public private(set) var runs: [LineRun] = []
    public private(set) var photoPins: [Coordinate] = []
    /// Where each day after a transfer starts, by the day before it.
    public private(set) var transferDestinations: [Int: String] = [:]
    /// The re-balance offer until the rider uses or dismisses it.
    public private(set) var offer: RebalanceOffer?
    /// "Furka 2,431 m · Biggest day 82.0 km".
    public private(set) var highlights: String?

    public let trip: Trip
    @ObservationIgnored private let rides: [RideSummary]
    @ObservationIgnored private let library: any LibraryStore
    @ObservationIgnored private let placeName: (@Sendable (Coordinate) async -> String?)?
    @ObservationIgnored private var started = false

    public init(
        trip: Trip, rides: [RideSummary], library: any LibraryStore,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil
    ) {
        self.trip = trip
        self.rides = rides.filter { $0.trip?.key == trip.key }
        self.library = library
        self.placeName = placeName
    }

    /// "Days 3–4 are longer now. Even them out?"
    public var offerTitle: String? {
        offer.map { offer in
            "Days \(offer.days.lowerBound + 1)–\(offer.days.upperBound + 1) are \(offer.shortfall > 0 ? "longer" : "shorter") now. Even them out?"
        }
    }

    public func start() async {
        guard !started else { return }
        started = true
        var tracks: [RideID: [RidePoint]] = [:]
        for ride in rides { tracks[ride.id] = library.ridePoints(ride.id) ?? [] }
        let (trip, rides) = (trip, rides)
        let loaded = tracks
        let computed = await Task.detached(priority: .userInitiated) {
            TripReview(trip: trip, rides: rides, tracks: loaded)
        }.value
        guard let review = computed else { return }

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
                day: index, header: header(index, day), note: library.dayNote(.tripDay(key: trip.key, dayIndex: index)),
                photos: photos, thumbnails: thumbnails, rides: day.rides)
        }
        photoPins = pins
        offer = review.rebalance.flatMap { library.rideJournal($0.ride).closedRows.contains(.rebalance) ? nil : $0 }
        highlights = Self.highlights(review, trip: trip)
        self.review = review
        await namePlaces(tracks)
    }

    /// The rider used or dismissed the offer: it never comes back for that day.
    public func closeOffer() {
        guard let offer else { return }
        var journal = library.rideJournal(offer.ride)
        journal.close(.rebalance)
        library.saveRideJournal(journal, thumbnails: [:], for: offer.ride)
        self.offer = nil
    }

    private func header(_ index: Int, _ day: TripReview.Day, from: String? = nil, to: String? = nil) -> String {
        OBCFormat.dayNoteHeader(
            date: day.rides[0].date, from: from ?? trip.dayStart(index)?.name, to: to ?? trip.dayEnds[index].name,
            distanceMeters: day.totals.distanceMeters)
    }

    /// Ask the geocoder for the places the trip does not name: a day's start after a transfer, or
    /// a day end the rider never named.
    private func namePlaces(_ tracks: [RideID: [RidePoint]]) async {
        guard let placeName, let review else { return }
        for (position, entry) in entries.enumerated() {
            let day = review.days[entry.day]
            let points = day.rides.flatMap { tracks[$0.id] ?? [] }
            var from = trip.dayStart(entry.day)?.name, to = trip.dayEnds[entry.day].name
            if from == nil, let first = points.first { from = await placeName(first.coordinate) }
            if to == nil, let last = points.last { to = await placeName(last.coordinate) }
            entries[position].header = header(entry.day, day, from: from, to: to)
        }
        for day in 0..<(trip.dayCount - 1) where trip.endsAtTransfer(day) {
            if let start = trip.dayStart(day + 1), let name = await placeName(start.coordinate) {
                transferDestinations[day] = name
            }
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
            parts.append(OBCFormat.highlight(.biggestDay(distance: review.days[biggest].totals.distanceMeters)))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
