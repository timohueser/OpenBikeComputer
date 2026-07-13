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
    func everyPaletteColorIsAThemeAccent() {
        // No color outside the OBC accent tokens (#240).
        let accents: Set<Color> = [
            OBCTheme.forest, OBCTheme.forestDeep, OBCTheme.wood,
            OBCTheme.amber, OBCTheme.coral, OBCTheme.water,
        ]
        for color in OBCTheme.stagePalette { #expect(accents.contains(color)) }
    }
}
