import Testing
import SwiftUI
@testable import OBCUI

/// The app-only stage palette: `stageColor(index:)` is a pure function of the stage's position
/// in ride order, and adjacent stages never share a colour.
struct StagePaletteTests {
    @Test
    func indexWrapsAroundThePalette() {
        let n = OBCTheme.stagePalette.count
        #expect(OBCTheme.stageColor(index: 0) == OBCTheme.stagePalette[0])
        #expect(OBCTheme.stageColor(index: n) == OBCTheme.stageColor(index: 0))
        #expect(OBCTheme.stageColor(index: n + 1) == OBCTheme.stageColor(index: 1))
        #expect(OBCTheme.stageColor(index: -3) == OBCTheme.stagePalette[0])
    }

    @Test
    func adjacentStagesDiffer() {
        for index in 0..<8 {
            #expect(OBCTheme.stageColor(index: index) != OBCTheme.stageColor(index: index + 1))
        }
    }
}
