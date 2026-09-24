import SwiftUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// The companion UI's tracked design-token authority: colours, radii and chrome metrics.
/// Reuse these values instead of introducing one-off styling. The component gallery and the
/// screenshot tests are the visual reference for how the tokens compose.
///
/// Every colour carries a Day and a Tent (dark) value and follows the system appearance. The
/// colours take the roles of the device's 64-colour palette, tuned for a phone. Amber and
/// magenta are never text colours.
public enum OBCTheme {
    // MARK: Surfaces
    /// The page behind everything.
    public static let page = Color(day: 0xF4F2EB, tent: 0x16150F)
    /// Grouped lists and cards.
    public static let surface = Color(day: 0xFFFFFF, tent: 0x201F17)
    /// Sheets, and a selected or raised row.
    public static let surface2 = Color(day: 0xF7F5EF, tent: 0x1E1D16)
    /// Sunken tracks: segmented controls, progress tracks, search fields, off pills.
    public static let fill = Color(day: 0x5C5A2E, tent: 0xBDB47E, opacity: 0.09, tentOpacity: 0.13)

    // MARK: Text
    /// Primary text.
    public static let ink = Color(day: 0x1C1B14, tent: 0xF2EFE3)
    /// Olive: captions, stat lines, secondary text and quiet icons.
    public static let secondary = Color(day: 0x5C5A2E, tent: 0xBDB47E)
    /// The control tint for links, text buttons and toggles.
    public static let tint = secondary

    // MARK: Lines
    /// Hairline separators and borders.
    public static let hairline = Color(day: 0x5C5A2E, tent: 0xBDB47E, opacity: 0.15)
    /// Outlines that must read on a map or a photo.
    public static let hairlineStrong = Color(day: 0x5C5A2E, tent: 0xBDB47E, opacity: 0.32)

    // MARK: Roles
    /// The one action on a screen: a fill under `onAmber` text, and the elevation line.
    public static let amber = Color(day: 0xF4A81D, tent: 0xF2A93A)
    /// Text and glyphs on `amber`.
    public static let onAmber = Color(day: 0x1C1B14, tent: 0x16150F)
    /// "On the device": the device's title-bar colour, as a fill under `onRust`.
    public static let rust = Color(day: 0xA4501E, tent: 0x93461A)
    /// Text and glyphs on `rust`.
    public static let onRust = Color(day: 0xFFF2D1, tent: 0xFFE9C0)
    /// A planned route line, always drawn over `routeCasing`.
    public static let route = Color(day: 0xCC2A93, tent: 0xE45CB5)
    /// The casing under a planned route line.
    public static let routeCasing = Color(day: 0xF4A81D, tent: 0xC98A1E)
    /// A recorded ride.
    public static let ride = Color(day: 0x2D3E96, tent: 0x8C9BF0)
    /// The second trip day, next to `route` for the first.
    public static let day2 = Color(day: 0x2F6FB5, tent: 0x6FA8E8)
    /// Failures and destructive actions.
    public static let danger = Color(day: 0xB0301C, tent: 0xF28B6E)

    // MARK: Sketch
    /// The ground under a drawn track sketch.
    public static let sketchGround = Color(day: 0xE7ECDF, tent: 0x232820)
    /// The faint grid on a sketch or a profile.
    public static let sketchLine = Color(day: 0x3E6E40, tent: 0xAAC8A0, opacity: 0.10, tentOpacity: 0.07)
    /// The area under an elevation line.
    public static let profileFill = Color(day: 0xE3DFC4, tent: 0x34331F)

    // MARK: Device illustration
    // The little hardware drawing on the launch and pairing screens. These draw the device, not
    // app chrome, so they keep the exact device colours in both appearances.
    /// Device upper shell: the "Forest" colourway of the current industrial design.
    public static let deviceBody = Color(hex: 0x2F6350)
    /// Device lower shell: the "Celadon" base the body is seated on, showing as a lip.
    public static let deviceAccent = Color(hex: 0x8BE3BC)
    /// The four device buttons: dark moulded rubber.
    public static let deviceButton = Color(hex: 0x2B2F36)
    /// The deep black bezel the panel is recessed into.
    public static let deviceBezel = Color(hex: 0x101317)
    /// The device UI's rust title bar.
    public static let deviceHeader = Color(hex: 0xAA5500)
    /// Cream text on the title bar.
    public static let deviceHeaderText = Color(hex: 0xFFFFAA)
    /// The device screen's track amber, from the on-glass palette.
    public static let deviceTrack = Color(hex: 0xFFAA00)

    // MARK: Radii
    /// Buttons and inputs.
    public static let controlRadius: CGFloat = 14
    /// Badges, pills and small controls.
    public static let radiusSmall: CGFloat = 7
    /// Inputs, the segmented track and banners.
    public static let radiusMedium: CGFloat = 11
    /// Grouped-list bodies, and track and profile cards.
    public static let radiusPanel: CGFloat = 14
    /// Route cards.
    public static let radiusCard: CGFloat = 16
    /// Large feature panels.
    public static let radiusLarge: CGFloat = 20
    /// Bottom-sheet top corners.
    public static let radiusSheet: CGFloat = 22
}

extension Color {
    /// A 24-bit sRGB literal, `0xRRGGBB`, the same in both appearances.
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }

    /// A colour that resolves to `day` in the light appearance and to `tent` in the dark one.
    init(day: UInt32, tent: UInt32, opacity: Double = 1, tentOpacity: Double? = nil) {
        let dayColor = PlatformColor(hex: day, alpha: opacity)
        let tentColor = PlatformColor(hex: tent, alpha: tentOpacity ?? opacity)
        #if canImport(UIKit)
        self.init(uiColor: UIColor { $0.userInterfaceStyle == .dark ? tentColor : dayColor })
        #else
        self.init(nsColor: NSColor(name: nil) {
            $0.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? tentColor : dayColor
        })
        #endif
    }
}

#if canImport(UIKit)
private typealias PlatformColor = UIColor
#else
private typealias PlatformColor = NSColor
#endif

private extension PlatformColor {
    convenience init(hex: UInt32, alpha: Double) {
        self.init(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: alpha
        )
    }
}
