import Foundation
import Testing
import OBCDomain
@testable import OBCUI

/// Route auto-expiry formatting (epic #638 S7): the retention labels, the
/// detail's "Expires …" line, and the library card's countdown badge. Pinned to a
/// fixed `now` in UTC/en_US so the day-granularity boundaries (exactly 3 days,
/// < 24 h, the badge window edge) and the absolute-date fallback are exact.
@Suite struct RouteExpiryFormatTests {
    private let en = Locale(identifier: "en_US")
    /// A UTC gregorian calendar so the absolute-date fallback ("Expires Jul 25")
    /// doesn't drift with the test machine's timezone.
    private var cal: Calendar {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        cal.locale = en
        return cal
    }
    /// 2026-07-15 12:00 UTC.
    private var now: Date {
        cal.date(from: DateComponents(year: 2026, month: 7, day: 15, hour: 12))!
    }

    private func daysOut(_ interval: TimeInterval) -> Date {
        now.addingTimeInterval(interval)
    }

    // MARK: Retention labels

    @Test func retentionLabelsMatchTheDesignList() {
        #expect(OBCFormat.retentionLabel(.never) == "Never")
        #expect(OBCFormat.retentionLabel(.oneDay) == "After 1 day")
        #expect(OBCFormat.retentionLabel(.oneWeek) == "After 1 week")
        #expect(OBCFormat.retentionLabel(.twoWeeks) == "After 2 weeks")
        #expect(OBCFormat.retentionLabel(.oneMonth) == "After 1 month")
        #expect(OBCFormat.retentionLabel(.twoMonths) == "After 2 months")
    }

    /// The picker order is the design's Never → 2 months.
    @Test func retentionCasesAreInDesignOrder() {
        #expect(Retention.allCases == [.never, .oneDay, .oneWeek, .twoWeeks, .oneMonth, .twoMonths])
    }

    // MARK: Detail expiry line

    @Test func detailLineIsRelativeInsideTheWindow() {
        #expect(OBCFormat.routeExpiry(daysOut(2 * 86_400 + 3_600), relativeTo: now, calendar: cal, locale: en)
            == "Expires in 2 days")
    }

    @Test func detailLineSaysTodayUnder24h() {
        #expect(OBCFormat.routeExpiry(daysOut(6 * 3_600), relativeTo: now, calendar: cal, locale: en)
            == "Expires today")
    }

    @Test func detailLineIsSingularAtOneDay() {
        // 1 day + 2 h floors to 1 → "in 1 day", not "in 1 days".
        #expect(OBCFormat.routeExpiry(daysOut(86_400 + 2 * 3_600), relativeTo: now, calendar: cal, locale: en)
            == "Expires in 1 day")
    }

    @Test func detailLineFallsBackToAnAbsoluteDateBeyondTheWindow() {
        // 10 days out → beyond the ≤3-day window → the "MMM d" date (2026-07-25).
        #expect(OBCFormat.routeExpiry(daysOut(10 * 86_400), relativeTo: now, calendar: cal, locale: en)
            == "Expires Jul 25")
    }

    // MARK: Card countdown badge (≤ 3 days only)

    @Test func badgeShowsInsideTheThreeDayWindow() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(2 * 86_400 + 3_600), relativeTo: now)
            == "Expires in 2 days")
    }

    /// Exactly 3 days is inside the window (inclusive) — the boundary the issue
    /// calls out.
    @Test func badgeShowsAtExactlyThreeDays() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(3 * 86_400), relativeTo: now) == "Expires in 3 days")
    }

    /// Just past 3 days still floors to 3 (day granularity) — the badge holds
    /// until the whole third day elapses.
    @Test func badgeHoldsJustPastThreeDays() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(3 * 86_400 + 3_600), relativeTo: now)
            == "Expires in 3 days")
    }

    /// Four days out (floor 4) is beyond the window — no badge on the card.
    @Test func badgeHidesBeyondTheWindow() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(4 * 86_400), relativeTo: now) == nil)
    }

    @Test func badgeSaysTodayUnder24h() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(6 * 3_600), relativeTo: now) == "Expires today")
    }

    /// A past expiry (the reconcile removes it before it renders, but be safe)
    /// reads "today", never a negative count.
    @Test func badgeClampsAPastExpiryToToday() {
        #expect(OBCFormat.routeExpiryBadge(daysOut(-3_600), relativeTo: now) == "Expires today")
    }

    @Test func badgeIsNilForNoExpiry() {
        #expect(OBCFormat.routeExpiryBadge(nil, relativeTo: now) == nil)
    }
}
