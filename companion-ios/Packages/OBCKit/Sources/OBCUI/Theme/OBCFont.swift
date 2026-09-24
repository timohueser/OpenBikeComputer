import SwiftUI

/// The companion type is SF Pro at system text styles, so every size follows Dynamic Type:
/// titles and body take a text style directly, as in `.system(.title2, weight: .bold)`. The
/// device's own pixel font appears only where the device speaks, through `PixelText`.
public extension Font {
    /// A stat value: SF Pro semibold with tabular figures.
    static func obcStat(_ style: Font.TextStyle) -> Font {
        .system(style, weight: .semibold).monospacedDigit()
    }
}

/// A caption label: SF Pro semibold, uppercase and tracked, in the secondary colour, as in
/// "ELEVATION".
public struct OBCEyebrow: View {
    let text: String

    public init(_ text: String) { self.text = text }

    public var body: some View {
        Text(text.uppercased())
            .font(.system(.caption, weight: .semibold))
            .kerning(1)
            .foregroundStyle(OBCTheme.secondary)
    }
}

public extension View {
    /// Caps Dynamic Type for a glyph or label inside fixed geometry, such as an icon button, a
    /// badge or a map pin, where a larger size cannot fit. Text in the flow never takes this.
    func obcFixedGeometryType() -> some View {
        dynamicTypeSize(...DynamicTypeSize.xxLarge)
    }
}
