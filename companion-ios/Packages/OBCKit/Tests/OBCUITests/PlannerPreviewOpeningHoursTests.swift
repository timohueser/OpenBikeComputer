#if DEBUG
import Foundation
import Testing
@testable import OBCUI

struct PlannerPreviewOpeningHoursTests {
    private let calendar = Calendar(identifier: .gregorian)
    /// Wednesday 2026-09-30 at `hour`:`minute`.
    private func wednesday(_ hour: Int, _ minute: Int = 0) -> Date {
        calendar.date(from: DateComponents(year: 2026, month: 9, day: 30, hour: hour, minute: minute))!
    }
    private func sunday(_ hour: Int) -> Date {
        calendar.date(from: DateComponents(year: 2026, month: 10, day: 4, hour: hour))!
    }

    @Test func dayRangesAndSplitTimesDecideOpenOrClosed() {
        let spec = "Mo-Sa 08:00-12:00,16:00-20:00; Su 09:00-13:00"
        #expect(PlannerPreviewOpeningHours.isOpen(spec, at: wednesday(9), calendar: calendar) == true)
        #expect(PlannerPreviewOpeningHours.isOpen(spec, at: wednesday(13), calendar: calendar) == false)
        #expect(PlannerPreviewOpeningHours.isOpen(spec, at: sunday(10), calendar: calendar) == true)
        #expect(PlannerPreviewOpeningHours.isOpen(spec, at: sunday(14), calendar: calendar) == false)
    }

    @Test func laterRuleOverridesAndUnnamedDaysAreClosed() {
        #expect(PlannerPreviewOpeningHours.isOpen("Tu-Su 09:00-18:00", at: wednesday(10), calendar: calendar) == true)
        #expect(PlannerPreviewOpeningHours.isOpen("Tu-Sa 09:00-18:00", at: sunday(10), calendar: calendar) == false)
        #expect(PlannerPreviewOpeningHours.isOpen("08:00-19:00; We off", at: wednesday(10), calendar: calendar) == false)
        #expect(PlannerPreviewOpeningHours.isOpen("Sa-Mo 22:00-02:00", at: sunday(1), calendar: calendar) == true)
        #expect(PlannerPreviewOpeningHours.isOpen("24/7", at: sunday(3), calendar: calendar) == true)
    }

    @Test func unknownShapesStaySilent() {
        #expect(PlannerPreviewOpeningHours.isOpen("sunrise-sunset", at: wednesday(10), calendar: calendar) == nil)
        #expect(PlannerPreviewOpeningHours.isOpen("Mo-Fr 09:00-17:00; PH off", at: wednesday(10), calendar: calendar) == nil)
        #expect(PlannerPreviewOpeningHours.isOpen("", at: wednesday(10), calendar: calendar) == nil)
    }
}
#endif
