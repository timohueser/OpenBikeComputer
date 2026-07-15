import Foundation
import Testing
import OBCDomain

/// The `Retention` domain enum (epic #638): the wire levels 0–5, their `days`
/// mapping, and the safe decode that sanitises an unknown byte to `.never` (the
/// firmware's posture — a forward-compat value must never surprise-delete).
@Suite struct RetentionTests {
    /// The wire mapping is the epic's locked table (`u8` → level).
    @Test(arguments: zip(
        [UInt8(0), 1, 2, 3, 4, 5],
        [Retention.never, .oneDay, .oneWeek, .twoWeeks, .oneMonth, .twoMonths]))
    func wireValues(_ raw: UInt8, _ level: Retention) {
        #expect(Retention(rawValue: raw) == level)
        #expect(level.rawValue == raw)
    }

    /// `days` mirrors the locked table; `never` has no finite expiry.
    @Test func daysAccessor() {
        #expect(Retention.never.days == nil)
        #expect(Retention.oneDay.days == 1)
        #expect(Retention.oneWeek.days == 7)
        #expect(Retention.twoWeeks.days == 14)
        #expect(Retention.oneMonth.days == 30)
        #expect(Retention.twoMonths.days == 60)
    }

    /// A known byte decodes to its level; an **unknown** byte (6, 255, …)
    /// sanitises to `.never` — never a trap, never a delete.
    @Test(arguments: [UInt8(6), 7, 42, 255])
    func unknownByteIsNever(_ raw: UInt8) {
        #expect(Retention(safeRawValue: raw) == .never)
    }

    @Test func knownByteRoundTripsThroughSafeDecode() {
        for level in Retention.allCases {
            #expect(Retention(safeRawValue: level.rawValue) == level)
        }
    }

    @Test func sixLevelsAndATwoWeekDefault() {
        #expect(Retention.allCases.count == 6)
        #expect(Retention.appDefault == .twoWeeks)
    }
}
