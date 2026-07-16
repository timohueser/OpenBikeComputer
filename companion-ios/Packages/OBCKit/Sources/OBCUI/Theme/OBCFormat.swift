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

    /// Trip card stat line: "2 stages · 141 km · 2,050 m ↑" (TR6) — the summed
    /// distance/climb over a trip's resolvable stages, led by the stage count.
    public static func tripSubtitle(
        stageCount: Int,
        distanceMeters: Double,
        elevationGainMeters: Double,
        locale: Locale = .current
    ) -> String {
        [
            stageCount == 1 ? "1 stage" : "\(stageCount) stages",
            distance(meters: distanceMeters, locale: locale),
            climb(meters: elevationGainMeters, locale: locale),
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

    // ------------------------------------------------------------- retention (epic #638)

    /// The picker/value label for a retention level (S7): "Never", "After 1 day",
    /// "After 1 week", "After 2 weeks", "After 1 month", "After 2 months". Shared
    /// by the Settings default row, the upload sheet's Auto-delete row, and the
    /// route-detail control, so every surface reads the level identically.
    public static func retentionLabel(_ retention: Retention) -> String {
        switch retention {
        case .never: "Never"
        case .oneDay: "After 1 day"
        case .oneWeek: "After 1 week"
        case .twoWeeks: "After 2 weeks"
        case .oneMonth: "After 1 month"
        case .twoMonths: "After 2 months"
        }
    }

    /// The route-detail expiry line from the device's `expires_at` (S7): the near
    /// form "Expires today" / "Expires in 2 days" inside the badge window, the
    /// absolute "Expires Jul 23" beyond it. Day granularity by design — the device
    /// extends expiry on use, so a live countdown would lie; "today" covers the
    /// last day (< 24 h). Deliberately approximate, matching the device's own
    /// "Auto-delete · in 12 d". `now` is injectable so tests can pin it.
    public static func routeExpiry(
        _ expiresAt: Date,
        relativeTo now: Date = Date(),
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String {
        let days = expiryDaysAway(expiresAt, from: now)
        if days <= Self.expiryBadgeDayWindow {
            return "Expires \(relativeExpiryPhrase(days: days))"
        }
        var calendar = calendar
        calendar.locale = locale
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.setLocalizedDateFormatFromTemplate("MMM d")
        return "Expires \(formatter.string(from: expiresAt))"
    }

    /// The library-card countdown footnote (S7): non-nil **only** when the route
    /// is expiring within the badge window (≤ 3 days), so a far-off expiry shows
    /// nothing on the card — "Expires today" / "Expires in 2 days". Same near form
    /// as ``routeExpiry(_:relativeTo:calendar:locale:)`` so the card and the detail
    /// agree in that window.
    public static func routeExpiryBadge(
        _ expiresAt: Date?,
        relativeTo now: Date = Date()
    ) -> String? {
        guard let expiresAt else { return nil }
        let days = expiryDaysAway(expiresAt, from: now)
        guard days <= Self.expiryBadgeDayWindow else { return nil }
        return "Expires \(relativeExpiryPhrase(days: days))"
    }

    /// The library card shows the countdown only inside this many days of expiry
    /// (epic #638: "only when ≤ 3 days"); the detail uses the same window to pick
    /// the relative phrasing over an absolute date.
    static let expiryBadgeDayWindow = 3

    /// Whole days until `expiresAt`, floored and never negative — 0 for anything
    /// inside the last 24 h (or already past, which the reconcile removes before
    /// it can render). Floored so "in 2 days" holds until the 2-day mark passes.
    private static func expiryDaysAway(_ expiresAt: Date, from now: Date) -> Int {
        let seconds = expiresAt.timeIntervalSince(now)
        guard seconds > 0 else { return 0 }
        return Int(seconds / 86_400)
    }

    /// "today" / "in 1 day" / "in N days" — the tail of the near-expiry phrase.
    private static func relativeExpiryPhrase(days: Int) -> String {
        switch days {
        case ..<1: "today"
        case 1: "in 1 day"
        default: "in \(days) days"
        }
    }

    // ------------------------------------------------------------- transfers (B5/B7)

    /// "2.3" from bytes — plain-English megabytes, one decimal (pair with "MB").
    public static func megabytesValue(_ bytes: Int, locale: Locale = .current) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = 1
        formatter.minimumFractionDigits = 1
        let mb = Double(max(0, bytes)) / 1_000_000
        return formatter.string(from: NSNumber(value: mb)) ?? "\(mb)"
    }

    /// The unit the transfer readout uses, chosen from the total: real OBCR routes
    /// are tens of kB (MB would read "0.0"); rides/large payloads stay in MB.
    private static func transferUnit(forTotalBytes total: Int) -> (label: String, divisor: Double, decimals: Int) {
        total >= 1_000_000 ? ("MB", 1_000_000, 1) : ("kB", 1_000, 0)
    }

    private static func sizeValue(_ bytes: Int, divisor: Double, decimals: Int, locale: Locale) -> String {
        let formatter = numberFormatter(locale: locale)
        formatter.maximumFractionDigits = decimals
        formatter.minimumFractionDigits = decimals
        return formatter.string(from: NSNumber(value: Double(max(0, bytes)) / divisor)) ?? "0"
    }

    /// The upload sheet's size readout: "1.4 / 2.3 MB · route + waypoints" for a
    /// large payload, "18 / 24 kB · route" for a real OBCR route (design F) —
    /// never raw byte counts, and never a misleading "0.0 MB".
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

    /// The upload confirm's size line (S7 `.ready`): "24 kB · route + waypoints" —
    /// the total only, no running "done /" prefix (nothing has moved yet).
    public static func transferTotalSizeLine(
        totalBytes: Int,
        hasWaypoints: Bool,
        locale: Locale = .current
    ) -> String {
        let unit = transferUnit(forTotalBytes: totalBytes)
        let total = sizeValue(totalBytes, divisor: unit.divisor, decimals: unit.decimals, locale: locale)
        return "\(total) \(unit.label) · \(hasWaypoints ? "route + waypoints" : "route")"
    }

    private static func numberFormatter(locale: Locale) -> NumberFormatter {
        let formatter = NumberFormatter()
        formatter.locale = locale
        formatter.numberStyle = .decimal
        return formatter
    }
}
