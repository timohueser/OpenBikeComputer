import SwiftUI

/// Placeholder theme. The real component kit + full token → `Color`/`Font`
/// mapping is **B11**; B0 exposes just the forest tint so the scaffold view has
/// something branded to draw. Token source of truth:
/// `project/_ds/openbikecomputer-design-system-*/tokens/` and the iOS additions
/// in `project/OBC Companion App.dc.html` (see companion-ios/CLAUDE.md).
public enum OBCTheme {
    /// `--forest` #3c6b39 — the app tint.
    public static let tint = Color(red: 0x3C / 255, green: 0x6B / 255, blue: 0x39 / 255)

    /// `--parchment` #ece8cf — page base.
    public static let parchment = Color(red: 0xEC / 255, green: 0xE8 / 255, blue: 0xCF / 255)

    /// `--ink` #24331c — primary text.
    public static let ink = Color(red: 0x24 / 255, green: 0x33 / 255, blue: 0x1C / 255)
}
