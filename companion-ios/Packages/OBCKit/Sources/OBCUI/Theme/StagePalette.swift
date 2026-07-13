import OBCDomain
import SwiftUI

extension OBCTheme {
    /// The deterministic **stage-color palette** for a trip's routes (epic #526,
    /// locked decision): a stage's color is a pure function of its **index** in
    /// the trip's ride order — the trip card draws all stages on one map preview
    /// in these colors, and the stage list / detail chips match by position.
    ///
    /// **App-only, never persisted** (and the 3-bit device never knows — it draws
    /// one route at a time). Sourced entirely from the existing ``OBCTheme``
    /// accent tokens (no new colors, per #240): a fixed cycle that stays legible
    /// on parchment and cycles for a trip with more stages than distinct hues.
    ///
    /// Ordered so the **early** stages — where most trips live — are maximally
    /// separable: the dark pairs (`forest`/`water`, both low-luminance) sit at
    /// opposite ends of the common range. `water` at index 2 next to `forest` at
    /// index 0 was the on-glass complaint (2026-07-13): on a thin divider bar the
    /// dark green and the dark teal read as the same color.
    public static let stagePalette: [Color] = [forest, coral, amber, water, wood, forestDeep]

    /// The color for the stage at `index` in ride order — pure, deterministic,
    /// and wrapping (`index % palette.count`) so any stage count resolves. A
    /// negative index is clamped to the first color.
    public static func stageColor(index: Int) -> Color {
        guard index > 0 else { return stagePalette[0] }
        return stagePalette[index % stagePalette.count]
    }
}
