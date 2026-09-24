import OBCDomain
import SwiftUI

extension OBCTheme {
    /// The stage colours for a trip's routes, in rotation. A stage's colour is a pure function
    /// of its index in the trip's ride order, so the trip map, the stage list and the detail
    /// chips match by position.
    ///
    /// Each reads at 3:1 or more on the sketch ground and on the map, in Day and Tent. Blue and
    /// violet, the closest pair for deuteranopes, are never adjacent. App-only and never
    /// persisted; the device draws one route at a time.
    public static let stagePalette: [Color] = [route, day2, day3, day4]

    /// The colour for the stage at `index` in ride order. It wraps, so any stage count
    /// resolves; a negative index clamps to the first colour.
    public static func stageColor(index: Int) -> Color {
        guard index > 0 else { return stagePalette[0] }
        return stagePalette[index % stagePalette.count]
    }
}
