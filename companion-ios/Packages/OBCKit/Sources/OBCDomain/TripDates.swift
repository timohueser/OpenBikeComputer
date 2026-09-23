import Foundation

/// A calendar day without a time or a time zone: days since 1970-01-01. The trip object carries
/// the start date in this unit, and the rider sees it as a weekday.
public struct CivilDay: Hashable, Comparable, Sendable {
    public var daysSince1970: Int

    public init(daysSince1970: Int) {
        self.daysSince1970 = daysSince1970
    }

    /// The calendar day that `date` falls on in `calendar`, which carries the rider's time zone.
    public init(_ date: Date, calendar: Calendar = .current) {
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        let midnight = Self.utc.date(from: DateComponents(year: parts.year, month: parts.month, day: parts.day)) ?? date
        self.init(daysSince1970: Int((midnight.timeIntervalSince1970 / 86_400).rounded(.down)))
    }

    /// The start of this day in `calendar`.
    public func date(calendar: Calendar = .current) -> Date {
        let utcMidnight = Date(timeIntervalSince1970: TimeInterval(daysSince1970) * 86_400)
        let parts = Self.utc.dateComponents([.year, .month, .day], from: utcMidnight)
        return calendar.date(from: parts) ?? utcMidnight
    }

    public static func + (day: CivilDay, days: Int) -> CivilDay {
        CivilDay(daysSince1970: day.daysSince1970 + days)
    }

    public static func < (lhs: CivilDay, rhs: CivilDay) -> Bool {
        lhs.daysSince1970 < rhs.daysSince1970
    }

    private static let utc: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }()
}

extension Trip {
    /// The date of each day. Dates follow the rides: day `k` takes the latest day `j ≤ k` that a
    /// ride finished, and adds `k − j`. Before any ride, day `k` is the start date plus `k`.
    /// Without a start date or a ride, a day has no date. `finished` maps a day index to the
    /// date a ride of that day finished.
    public func dayDates(finished: [Int: CivilDay] = [:]) -> [CivilDay?] {
        var anchor: (day: Int, date: CivilDay)? = startDay.map { (0, $0) }
        return (0..<dayCount).map { k in
            if let ride = finished[k] { anchor = (k, ride) }
            return anchor.map { $0.date + (k - $0.day) }
        }
    }
}
