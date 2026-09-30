#if DEBUG
import Testing
@testable import OBCUI

struct PlannerPreviewGradeTests {
    @Test func distanceWindowSmoothsUnevenSamplesWithoutBridgingMissingTerrain() {
        let rising = PlannerPreviewGrade(samples: [0.0, 10, 25, 100, 400, 900, 1_000].map {
            .init(distance: $0, elevation: $0 / 10)
        })
        #expect(rising.grades.allSatisfy { abs(($0 ?? 0) - 10) < 0.0001 })
        let ripple = PlannerPreviewGrade(samples: (0...20).map {
            .init(distance: Double($0 * 10), elevation: $0 == 10 ? 1 : 0)
        })
        #expect(ripple.grades.allSatisfy { abs($0 ?? 0) <= 1 })
        let gaps = PlannerPreviewGrade(samples: [
            .init(distance: 0, elevation: 0), .init(distance: 100, elevation: 10),
            .init(distance: 200, elevation: nil), .init(distance: 300, elevation: 400), .init(distance: 500, elevation: 420),
        ])
        #expect(gaps.grades == [10, nil, nil, 10])
        let short = PlannerPreviewGrade(samples: [.init(distance: 0, elevation: 0), .init(distance: 10, elevation: 5)])
        #expect(short.grades == [nil])
    }

    @Test func colorsMatchDesktopThresholdsInBothDirections() {
        let grades: [Double?] = [-50, -20, -15, -10, -6, -3, 0, 3, 6, 10, 15, 20, 50, nil]
        #expect(grades.map(PlannerPreviewGrade.band) == [0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 10, 11])
        for grade in [2.94, 2.96, 6, 9.99999, 15, 19.96, 50] {
            #expect(PlannerPreviewGrade.band(-grade) + PlannerPreviewGrade.band(grade) == 10)
        }
        #expect(PlannerPreviewGrade.label(-10.25) == "−10.2%")
        #expect(PlannerPreviewGrade.label(-0.01) == "0.0%")
    }

    @Test func viewportCropKeepsAbsoluteDistanceAndInterpolatedElevations() throws {
        let profile = PlannerPreviewGrade(samples: [
            .init(distance: 0, elevation: 0), .init(distance: 100, elevation: 10), .init(distance: 400, elevation: 40),
        ])
        let segments = profile.segments(in: 0.125...0.75)
        #expect(segments.count == 2)
        let first = try #require(segments.first), last = try #require(segments.last)
        #expect(first.from == 0.125 && first.start == 5)
        #expect(last.to == 0.75 && last.end == 30)
        #expect(profile.reading(at: 0.5).elevation == 20)
        #expect(profile.reading(at: 0.5).grade == 10)
        #expect(profile.segments(in: 0.3...0.3).isEmpty)
    }
}
#endif
