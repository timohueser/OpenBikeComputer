import Testing
import SwiftUI
@testable import OBCUI

/// TR5 — the app-only stage palette: `stageColor(index:)` is a pure, deterministic
/// function of the stage's position in ride order (never persisted; the device
/// never knows). Sourced entirely from `OBCTheme` accent tokens (no new colors).
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
        // The on-glass feedback (2026-07-13): forest (stage 1) and water (stage 3)
        // were near-indistinguishable on a thin divider bar. The first three
        // stages — where most trips live — must be the high-contrast trio; the
        // dark teal may not appear before index 3.
        #expect(Array(OBCTheme.stagePalette.prefix(3)) == [OBCTheme.forest, OBCTheme.coral, OBCTheme.amber])
        #expect(OBCTheme.stagePalette.firstIndex(of: OBCTheme.water).map { $0 >= 3 } == true)
    }

    @Test
    func everyPaletteColorIsAThemeAccent() {
        // No color outside the OBC accent tokens (#240).
        let accents: Set<Color> = [
            OBCTheme.forest, OBCTheme.forestDeep, OBCTheme.wood,
            OBCTheme.amber, OBCTheme.coral, OBCTheme.water,
        ]
        for color in OBCTheme.stagePalette { #expect(accents.contains(color)) }
    }
}
