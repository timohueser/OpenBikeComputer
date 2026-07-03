import SwiftUI
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Brand type helpers. Three faces:
/// - **serif** — Iowan Old Style, the brand display face (ships with iOS/macOS;
///   Spectral is only the *web* stand-in). Large titles, headings, empty-state lines.
/// - **ui** — SF Pro via the system font. Body, controls, chrome.
/// - **mono** — the system monospaced face. Stat lines, eyebrow labels, values.
public extension Font {
    /// The field-guide serif. Falls back to the system serif design if Iowan
    /// Old Style is ever unavailable (it is bundled on iOS + macOS).
    static func obcSerif(size: CGFloat, weight: Font.Weight = .bold) -> Font {
        if hasIowan {
            return .custom("Iowan Old Style", size: size).weight(weight)
        }
        return .system(size: size, weight: weight, design: .serif)
    }

    /// Monospace for stat lines / labels / values.
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

/// The monospace eyebrow label — 10pt bold mono, uppercase, letter-spaced,
/// ink-faint ("ELEVATION PROFILE", "CONNECTED SERVICES").
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
