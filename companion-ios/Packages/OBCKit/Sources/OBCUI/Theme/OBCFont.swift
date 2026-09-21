import SwiftUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Brand type helpers for three faces: the Iowan Old Style serif for large titles and
/// headings, the system font for body and chrome, and the system monospace for stat
/// lines, eyebrow labels and values.
public extension Font {
    /// The field-guide serif. Falls back to the system serif design if Iowan Old
    /// Style is ever unavailable.
    static func obcSerif(size: CGFloat, weight: Font.Weight = .bold) -> Font {
        if hasIowan {
            return .custom("Iowan Old Style", size: size).weight(weight)
        }
        return .system(size: size, weight: weight, design: .serif)
    }

    /// Monospace for stat lines, labels and values.
    static func obcMono(size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }

    private static let hasIowan: Bool = {
        #if canImport(UIKit)
        UIFont(name: "IowanOldStyle-Roman", size: 12) != nil
        #elseif canImport(AppKit)
        NSFont(name: "IowanOldStyle-Roman", size: 12) != nil
        #else
        false
        #endif
    }()
}

/// The monospace eyebrow label: bold, uppercase and letter-spaced, as in
/// "ELEVATION PROFILE".
public struct OBCEyebrow: View {
    let text: String

    public init(_ text: String) { self.text = text }

    public var body: some View {
        Text(text.uppercased())
            .font(.obcMono(size: 10, weight: .bold))
            .kerning(1)
            .foregroundStyle(OBCTheme.inkFaint)
    }
}
