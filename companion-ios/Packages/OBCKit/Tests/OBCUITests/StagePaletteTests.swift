import Testing
import SwiftUI
@testable import OBCUI

/// The app-only stage palette: `stageColor(index:)` is a pure function of the stage's position
/// in ride order. It is never persisted and the device never knows it; the colors come from
/// `OBCTheme` accent tokens.
struct StagePaletteTests {
    @Test
    func sameIndexIsAlwaysTheSameColor() {
        #expect(OBCTheme.stageColor(index: 2) == OBCTheme.stageColor(index: 2))
        #expect(OBCTheme.stageColor(index: 0) == OBCTheme.stagePalette[0])
    }

    @Test
    func indexWrapsAroundThePalette() {
        let n = OBCTheme.stagePalette.count
        #expect(OBCTheme.stageColor(index: n) == OBCTheme.stageColor(index: 0))
        #expect(OBCTheme.stageColor(index: n + 1) == OBCTheme.stageColor(index: 1))
    }

    @Test
    func earlyStagesAvoidTheDarkGreenTealPair() {
        // Forest (stage 1) and water (stage 3) are near-indistinguishable on a thin divider bar,
        // so the first three stages must be the high-contrast trio: no dark teal before index 3.
        #expect(Array(OBCTheme.stagePalette.prefix(3)) == [OBCTheme.forest, OBCTheme.coral, OBCTheme.amber])
        #expect(OBCTheme.stagePalette.firstIndex(of: OBCTheme.water).map { $0 >= 3 } == true)
    }

    @Test
    func everyPaletteColorIsAThemeAccent() {
        // No color outside the OBC accent tokens.
        let accents: Set<Color> = [
            OBCTheme.forest, OBCTheme.forestDeep, OBCTheme.wood,
            OBCTheme.amber, OBCTheme.coral, OBCTheme.water,
        ]
        for color in OBCTheme.stagePalette { #expect(accents.contains(color)) }
    }
}
