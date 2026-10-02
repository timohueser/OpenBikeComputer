#if os(iOS)
import UIKit

// Keep these tokens aligned with builder/app/src/lib/planner/route-overlays.ts.
enum PlannerPreviewNetworkStyle {
    static func color(rank: Int, traits: UITraitCollection) -> UIColor {
        let colors: [UInt32] = traits.userInterfaceStyle == .dark
            ? [0xb0b8be, 0xa4cf67, 0x79b7f1, 0xc49de0]
            : [0x626a70, 0x4f8b24, 0x2368b5, 0x7c519c]
        let value = colors[min(3, max(0, rank))]
        return UIColor(red: CGFloat((value >> 16) & 255) / 255,
                       green: CGFloat((value >> 8) & 255) / 255,
                       blue: CGFloat(value & 255) / 255, alpha: 0.8)
    }

}
#endif
