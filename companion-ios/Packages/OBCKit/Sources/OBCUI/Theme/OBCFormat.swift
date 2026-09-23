import Foundation
import OBCDomain

/// Stat-line formatting for the mono lines on cards and stat strips: one place so
/// every screen renders "62.4 km · 840 m ↑ · 3h 20m" identically. Metric only.
/// The `locale` and `calendar` parameters exist so tests can pin them.
public enum OBCFormat {
    /// "62.4 km" under 100 km, "118 km" above.
    public static func distance(meters: Double, locale: Locale = .current) -> String {
        let km = meters / 1000
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = km < 100 ? 1 : 0
        formatter.minimumFractionDigits = km < 100 ? 1 : 0
        let value = formatter.string(from: NSNumber(value: km)) ?? "\(km)"
        return "\(value) km"
    }

    /// "480 m" under a kilometre, in steps of 10 m; ``distance(meters:locale:)`` above.
    public static func shortDistance(meters: Double, locale: Locale = .current) -> String {
        let meters = abs(meters)
        guard meters < 1000 else { return distance(meters: meters, locale: locale) }
        return "\(Int((meters / 10).rounded()) * 10) m"
    }

    /// "on the line" or "430 m off the line": how far a stop is from a trip line.
    public static func stopOffset(meters: Double, locale: Locale = .current) -> String {
        meters <= Trip.onLineMeters ? "on the line" : "\(shortDistance(meters: meters, locale: locale)) off the line"
    }

    /// "840 m ↑" or "1,240 m ↑": climb with grouping.
    public static func climb(meters: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 0
        formatter.usesGroupingSeparator = true
        let value = formatter.string(from: NSNumber(value: meters.rounded())) ?? "\(Int(meters))"
        return "\(value) m ↑"
    }

    /// Planned estimate: "3h 20m"; multi-day routes read "2 days". Minutes floor, as on the device.
    public static func estimatedDuration(_ interval: TimeInterval) -> String {
        let minutes = Int(interval / 60)
        if minutes >= 24 * 60 {
            let days = Int((Double(minutes) / (24 * 60)).rounded())
            return days == 1 ? "1 day" : "\(days) days"
        }
        let h = minutes / 60
        let m = minutes % 60
        if h == 0 { return "\(m)m" }
        return m == 0 ? "\(h)h" : "\(h)h \(m)m"
    }

    /// Tracked moving time as "2:51" (h:mm).
    public static func movingTime(_ interval: TimeInterval) -> String {
        let minutes = Int((interval / 60).rounded())
        return String(format: "%d:%02d", minutes / 60, minutes % 60)
    }

    /// "20.4 kph" from metres per second.
    public static func speed(mps: Double, locale: Locale = .current) -> String {
        let kph = mps * 3.6
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 1
        formatter.minimumFractionDigits = 1
        let value = formatter.string(from: NSNumber(value: kph)) ?? "\(kph)"
        return "\(value) kph"
    }

    /// Ride-day label: "Today" or "Yesterday", a short weekday inside the last week,
    /// then a short date beyond.
    public static func rideDay(
        _ date: Date,
        relativeTo now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        var calendar = calendar
        calendar.locale = locale
        if calendar.isDate(date, inSameDayAs: now) { return "Today" }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now),
            calendar.isDate(date, inSameDayAs: yesterday) {
            return "Yesterday"
        }
        let days = calendar.dateComponents(
            [.day],
            from: calendar.startOfDay(for: date),
            to: calendar.startOfDay(for: now)
        ).day ?? .max
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = calendar
        formatter.setLocalizedDateFormatFromTemplate(days < 7 && days >= 0 ? "EEE" : "MMM d")
        return formatter.string(from: date)
    }

    /// A trip day's date: "Mon 29 Sep".
    public static func tripDay(_ day: CivilDay, calendar: Calendar = .current, locale: Locale = .current) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.setLocalizedDateFormatFromTemplate("EEE d MMM")
        return formatter.string(from: day.date(calendar: calendar))
    }

    /// A trip's date range: "Mon 29 Sep – Wed 1 Oct", or one date for a one-day trip.
    public static func tripDates(_ first: CivilDay, _ last: CivilDay, calendar: Calendar = .current, locale: Locale = .current) -> String {
        let start = tripDay(first, calendar: calendar, locale: locale)
        return first == last ? start : "\(start) – \(tripDay(last, calendar: calendar, locale: locale))"
    }

    // MARK: Card subtitles

    /// Planned-route stat line: "62.4 km · 840 m ↑ · 3h 20m".
    public static func plannedSubtitle(_ route: RouteSummary, locale: Locale = .current) -> String {
        var parts = [
            distance(meters: route.distanceMeters, locale: locale),
            climb(meters: route.elevationGainMeters, locale: locale),
        ]
        if let estimate = route.estimatedDuration {
            parts.append(estimatedDuration(estimate))
        }
        return parts.joined(separator: " · ")
    }

    /// Tracked-ride stat line: "Yesterday · 58.2 km · 2:51 · 20.4 kph".
    public static func trackedSubtitle(
        _ ride: RideSummary,
        relativeTo now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        [
            rideDay(ride.date, relativeTo: now, calendar: calendar, locale: locale),
            distance(meters: ride.distanceMeters, locale: locale),
            movingTime(ride.movingTime),
            speed(mps: ride.averageSpeedMps, locale: locale),
        ].joined(separator: " · ")
    }

    /// Ride detail stat line: "74.3 km · 5:52 · 12.7 kph · 2,080 m ↑".
    public static func rideStatsLine(_ ride: RideSummary, locale: Locale = .current) -> String {
        [
            distance(meters: ride.distanceMeters, locale: locale),
            movingTime(ride.movingTime),
            speed(mps: ride.averageSpeedMps, locale: locale),
            climb(meters: ride.climbMeters, locale: locale),
        ].joined(separator: " · ")
    }

    /// One highlight: "Furka 2,431 m", "2,431 m at km 31", "18.0 km climb", "62 kph descent" or
    /// "Biggest day 82.0 km". Lengths use `distance(meters:)`, as every other km in the app.
    public static func highlight(_ highlight: RideHighlight, locale: Locale = .current) -> String {
        switch highlight {
        case .highestPoint(let elevation, let distance, let place):
            let height = "\(climbValue(meters: elevation, locale: locale)) m"
            if let place { return "\(place) \(height)" }
            return "\(height) at km \(Int((distance / 1000).rounded()))"
        case .longestClimb(_, let length):
            return "\(Self.distance(meters: length, locale: locale)) climb"
        case .fastestDescent(let speedMps):
            return "\(Int((speedMps * 3.6).rounded())) kph descent"
        case .biggestDay(let distance):
            return "Biggest day \(Self.distance(meters: distance, locale: locale))"
        }
    }

    /// Trip card stat line: "2 stages · 141 km · 2,050 m ↑".
    public static func tripSubtitle(
        dayCount: Int,
        distanceMeters: Double,
        elevationGainMeters: Double,
        locale: Locale = .current
    ) -> String {
        [
            dayCount == 1 ? "1 day" : "\(dayCount) days",
            distance(meters: distanceMeters, locale: locale),
            climb(meters: elevationGainMeters, locale: locale),
        ].joined(separator: " · ")
    }

    // MARK: Stat-strip parts

    // The detail stat strips render value and unit separately; these are the same
    // numbers as the joined lines above, without the unit.

    /// "62.4" under 100 km, "118" above. Pair with unit "km".
    public static func distanceValue(meters: Double, locale: Locale = .current) -> String {
        let km = meters / 1000
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = km < 100 ? 1 : 0
        formatter.minimumFractionDigits = km < 100 ? 1 : 0
        return formatter.string(from: NSNumber(value: km)) ?? "\(km)"
    }

    /// "840" or "1,240". Pair with unit "m".
    public static func climbValue(meters: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 0
        formatter.usesGroupingSeparator = true
        return formatter.string(from: NSNumber(value: meters.rounded())) ?? "\(Int(meters))"
    }

    /// "20.4" from metres per second. Pair with unit "kph".
    public static func speedValue(mps: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 1
        formatter.minimumFractionDigits = 1
        return formatter.string(from: NSNumber(value: mps * 3.6)) ?? "\(mps * 3.6)"
    }

    /// The ride subtitle line: "Yesterday, 8:12 AM".
    public static func rideDateLine(
        _ date: Date,
        relativeTo now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = calendar
        formatter.timeStyle = .short
        formatter.dateStyle = .none
        let time = formatter.string(from: date)
        return "\(rideDay(date, relativeTo: now, calendar: calendar, locale: locale)), \(time)"
    }
    /// "today", "in 1 day" or "in N days": the tail of the near-expiry phrase.
    private static func relativeExpiryPhrase(days: Int) -> String {
        switch days {
        case ..<1: "today"
        case 1: "in 1 day"
        default: "in \(days) days"
        }
    }

    // MARK: Transfers

    /// "2.3" from bytes: megabytes with one decimal. Pair with unit "MB".
    public static func megabytesValue(_ bytes: Int, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 1
        formatter.minimumFractionDigits = 1
        let mb = Double(max(0, bytes)) / 1_000_000
        return formatter.string(from: NSNumber(value: mb)) ?? "\(mb)"
    }

    /// The unit the transfer readout uses, chosen from the total: a real route is tens
    /// of kB and would read "0.0" in MB; rides and larger payloads stay in MB.
    private static func transferUnit(forTotalBytes total: Int) -> (label: String, divisor: Double, decimals: Int) {
        total >= 1_000_000 ? ("MB", 1_000_000, 1) : ("kB", 1_000, 0)
    }

    private static func sizeValue(_ bytes: Int, divisor: Double, decimals: Int, locale: Locale) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = decimals
        formatter.minimumFractionDigits = decimals
        return formatter.string(from: NSNumber(value: Double(max(0, bytes)) / divisor)) ?? "0"
    }

    /// The upload sheet's size readout: "1.4 / 2.3 MB · route + waypoints", or
    /// "18 / 24 kB · route" for a small route. Never raw bytes, never "0.0 MB".
    public static func transferSizeLine(
        bytesDone: Int,
        totalBytes: Int,
        hasWaypoints: Bool,
        locale: Locale = .current
    ) -> String {
        let unit = transferUnit(forTotalBytes: totalBytes)
        let done = sizeValue(bytesDone, divisor: unit.divisor, decimals: unit.decimals, locale: locale)
        let total = sizeValue(totalBytes, divisor: unit.divisor, decimals: unit.decimals, locale: locale)
        return "\(done) / \(total) \(unit.label) · \(hasWaypoints ? "route + waypoints" : "route")"
    }

    private static func numberFormatter(locale: Locale) -> NumberFormatter {
        let formatter = NumberFormatter()
        formatter.locale = locale
        formatter.numberStyle = .decimal
        return formatter
    }
}
