import Foundation
import Observation
import OBCDomain
import OBCTransport

/// A ride's day note: the prompt row, the entry on the ride detail and the writer.
///
/// The note is keyed by the trip day, so the rides of one day share it. The writer autosaves:
/// a keystroke schedules a save, and closing the writer saves at once. Opening the writer uses
/// the prompt row, so the row never comes back for this ride.
@MainActor @Observable
public final class DayNoteModel {
    /// The quiet row shows.
    public private(set) var offer = false
    /// The saved note; the writer edits `draft`.
    public private(set) var note = ""
    public var draft = ""
    /// "Tue 30 Sep · Andermatt → Ulrichen · 74 km"; the places fill in when they are known.
    public private(set) var header: String
    /// "Day 2" on a trip day, else the ride's name.
    public let title: String
    /// "How was Day 2?"
    public let prompt: String
    /// The row was dismissed and no note exists: the entry offers "Add a note" instead.
    public var dismissed: Bool { !offer && note.isEmpty && journal.closedRows.contains(.note) }

    public static let saveDelay: Duration = .milliseconds(600)

    let key: DayNoteKey
    @ObservationIgnored private let ride: RideSummary
    @ObservationIgnored private let points: [RidePoint]
    @ObservationIgnored private let library: any LibraryStore
    @ObservationIgnored private let placeName: (@Sendable (Coordinate) async -> String?)?
    @ObservationIgnored private var journal = RideJournal()
    @ObservationIgnored private var pendingSave: Task<Void, Never>?
    @ObservationIgnored private var started = false

    public init(
        ride: RideSummary, points: [RidePoint], library: any LibraryStore,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil
    ) {
        self.ride = ride
        self.points = points
        self.library = library
        self.placeName = placeName
        key = DayNoteKey(ride)
        title = ride.trip.map { "Day \($0.dayIndex + 1)" } ?? ride.name
        prompt = OBCFormat.notePrompt(ride)
        header = OBCFormat.dayNoteHeader(date: ride.date, distanceMeters: ride.distanceMeters)
    }

    /// Loads the note and the row, then fills the header's places. A host may build throwaway
    /// models on every render, so this waits for the live one.
    public func start() async {
        guard !started else { return }
        started = true
        journal = library.rideJournal(ride.id)
        note = library.dayNote(key)
        draft = note
        offer = journal.offersNote(note)
        let (from, to) = await places()
        header = OBCFormat.dayNoteHeader(date: ride.date, from: from, to: to, distanceMeters: ride.distanceMeters)
    }

    /// The rider opens the writer, from the row or the entry. The row is used up either way.
    public func openWriter() {
        draft = note
        closeOffer()
    }

    public func dismissOffer() {
        closeOffer()
    }

    /// A keystroke: save after a pause, so the store is not written on every character.
    public func draftChanged() {
        pendingSave?.cancel()
        pendingSave = Task { [weak self] in
            try? await Task.sleep(for: Self.saveDelay)
            guard !Task.isCancelled else { return }
            self?.save()
        }
    }

    /// Closing the writer: save now.
    public func save() {
        pendingSave?.cancel()
        pendingSave = nil
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard text != note else { return }
        note = text
        library.saveDayNote(text, for: key)
    }

    private func closeOffer() {
        guard offer else { return }
        offer = false
        journal.close(.note)
        library.saveRideJournal(journal, thumbnails: [:], for: ride.id)
    }

    /// The day's start and end places: the trip's day ends when the phone has the trip, else the
    /// localities of the ride's first and last points.
    private func places() async -> (String?, String?) {
        var from: String?, to: String?
        if let day = ride.trip, let trip = library.trips().first(where: { $0.key == day.key }),
           day.dayIndex < trip.dayEnds.count {
            from = day.dayIndex == 0 ? trip.startName : trip.dayEnds[day.dayIndex - 1].name
            to = trip.dayEnds[day.dayIndex].name
        }
        if let placeName, let first = points.first, let last = points.last {
            if from == nil { from = await placeName(first.coordinate) }
            if to == nil { to = await placeName(last.coordinate) }
        }
        return (from, to)
    }
}
