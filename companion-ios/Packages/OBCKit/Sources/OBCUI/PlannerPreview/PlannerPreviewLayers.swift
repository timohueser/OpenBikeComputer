#if os(iOS)
import SwiftUI

// Keep these tokens aligned with builder/app/src/lib/planner/route-overlays.ts.
enum PlannerPreviewNetworkStyle {
    private static let light: [UInt32] = [0x626a70, 0x4f8b24, 0x2368b5, 0x7c519c]
    private static let dark: [UInt32] = [0xb0b8be, 0xa4cf67, 0x79b7f1, 0xc49de0]
    /// The opaque network colours by rank, for the signed routes on the map.
    static let lines: [Color] = zip(light, dark).map { Color(day: $0, tent: $1) }

    static func color(rank: Int, traits: UITraitCollection) -> UIColor {
        let value = (traits.userInterfaceStyle == .dark ? dark : light)[min(3, max(0, rank))]
        return UIColor(red: CGFloat((value >> 16) & 255) / 255,
                       green: CGFloat((value >> 8) & 255) / 255,
                       blue: CGFloat(value & 255) / 255, alpha: 0.8)
    }

}
#endif
