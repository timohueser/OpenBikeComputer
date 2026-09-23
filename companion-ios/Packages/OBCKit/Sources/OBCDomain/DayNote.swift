import Foundation

/// Where a ride's day note lives. The rides of one trip day share one note; a ride without a
/// trip keeps its own. The key never changes with an edit, so a split or a merge never moves a
/// note: the part that keeps the ride's id keeps its note, and a trip day's note stays with the
/// day.
public enum DayNoteKey: Hashable, Sendable {
    case ride(RideID)
    case tripDay(key: UInt64, dayIndex: Int)

    public init(_ ride: RideSummary) {
        if let trip = ride.trip {
            self = .tripDay(key: trip.key, dayIndex: trip.dayIndex)
        } else {
            self = .ride(ride.id)
        }
    }
}
