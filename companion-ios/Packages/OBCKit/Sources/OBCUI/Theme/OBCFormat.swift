import Foundation
import OBCDomain

/// Stat-line formatting for the mono lines the design shows on cards and stat
/// strips — one place so every screen renders "62.4 km · 840 m ↑ · 3h 20m"
/// identically. Metric-only for now (the device is metric); a unit-preference
/// seam can wrap this later without touching call sites.
///
/// All functions take a `locale`/`calendar` so tests can pin them; production
/// call sites use the defaults.
public enum OBCFormat {
    /// "62.4 km" under 100 km, "118 km" above (design C1 rows).
    public static func distance(meters: Double, locale: Locale = .current) -> String {
        let km = meters / 1000
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = km < 100 ? 1 : 0
        formatter.minimumFractionDigits = km < 100 ? 1 : 0
        let value = formatter.string(from: NSNumber(value: km)) ?? "\(km)"
        return "\(value) km"
    }

    /// "840 m ↑" / "1,240 m ↑" — climb with grouping (design C1 rows).
    public static func climb(meters: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 0
        formatter.usesGroupingSeparator = true
        let value = formatter.string(from: NSNumber(value: meters.rounded())) ?? "\(Int(meters))"
        return "\(value) m ↑"
    }

    /// Planned estimate: "3h 20m"; multi-day routes read "2 days" (C1's
    /// overnighter row).
    public static func estimatedDuration(_ interval: TimeInterval) -> String {
        let minutes = Int((interval / 60).rounded())
        if minutes >= 24 * 60 {
            let days = Int((Double(minutes) / (24 * 60)).rounded())
            return days == 1 ? "1 day" : "\(days) days"
        }
        let h = minutes / 60
        let m = minutes % 60
        if h == 0 { return "\(m)m" }
        return m == 0 ? "\(h)h" : "\(h)h \(m)m"
    }

    /// Tracked moving time as "2:51" (h:mm — the design's ride rows).
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

    /// Ride-day label the tracked rows lead with: "Today" / "Yesterday", a short
    /// weekday inside the last week ("Sun", "Fri"), then a short date beyond.
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

    // ------------------------------------------------------------- card subtitles
    /// Planned-route stat line: "62.4 km · 840 m ↑ · 3h 20m" (C1).
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

    /// Tracked-ride stat line: "Yesterday · 58.2 km · 2:51 · 20.4 kph" (C2).
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

    // ------------------------------------------------------------- stat-strip parts
    // The detail stat strips (E1–E3) render value and unit separately (`OBCStat`);
    // these are the same numbers the joined lines above use, without the unit.

    /// "62.4" under 100 km, "118" above — pair with unit "km".
    public static func distanceValue(meters: Double, locale: Locale = .current) -> String {
        let km = meters / 1000
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = km < 100 ? 1 : 0
        formatter.minimumFractionDigits = km < 100 ? 1 : 0
        return formatter.string(from: NSNumber(value: km)) ?? "\(km)"
    }

    /// "840" / "1,240" — pair with unit "m".
    public static func climbValue(meters: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 0
        formatter.usesGroupingSeparator = true
        return formatter.string(from: NSNumber(value: meters.rounded())) ?? "\(Int(meters))"
    }

    /// "20.4" from metres per second — pair with unit "kph".
    public static func speedValue(mps: Double, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 1
        formatter.minimumFractionDigits = 1
        return formatter.string(from: NSNumber(value: mps * 3.6)) ?? "\(mps * 3.6)"
    }

    /// E3's subtitle line: "Yesterday, 8:12 AM".
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

    private static func numberFormatter(locale: Locale) -> NumberFormatter {
        let formatter = NumberFormatter()
        formatter.locale = locale
        formatter.numberStyle = .decimal
        return formatter
    }
}
