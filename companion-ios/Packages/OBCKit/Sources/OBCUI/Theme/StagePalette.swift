import OBCDomain
import SwiftUI

extension OBCTheme {
    /// The deterministic stage-color palette for a trip's routes: a stage's color is
    /// a pure function of its index in the trip's ride order, so the trip card's map
    /// preview, the stage list and the detail chips all match by position.
    ///
    /// App-only, never persisted; the device draws one route at a time. Sourced from
    /// the existing ``OBCTheme`` accent tokens, and cycled for a trip with more stages
    /// than distinct hues.
    ///
    /// Ordered so the early stages, where most trips live, are maximally separable.
    /// `forest` and `water` are both dark and low-luminance: next to each other on a
    /// thin divider bar they read as the same color.
    public static let stagePalette: [Color] = [forest, coral, amber, water, wood, forestDeep]

    /// The color for the stage at `index` in ride order. It wraps, so any stage count
    /// resolves; a negative index clamps to the first color.
    public static func stageColor(index: Int) -> Color {
        guard index > 0 else { return stagePalette[0] }
        return stagePalette[index % stagePalette.count]
    }
}
