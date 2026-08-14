import SwiftUI

/// The companion UI's tracked design-token authority: colors, chrome metrics,
/// radii, and spacing. Reuse these values from views and components instead of
/// introducing one-off styling. The component gallery and screenshot tests are
/// the visual reference for how the tokens compose.
public enum OBCTheme {
    // ------------------------------------------------------------- base
    /// `--parchment` #ece8cf — page base, moss-tinted cream.
    public static let parchment = Color(hex: 0xECE8CF)
    /// `--parchment-2` #e4dec0 — raised surface.
    public static let parchment2 = Color(hex: 0xE4DEC0)
    /// `--parchment-3` #d6cda8 — sunken / tile / pill.
    public static let parchment3 = Color(hex: 0xD6CDA8)
    /// `--panel` #f3f0df — card face.
    public static let panel = Color(hex: 0xF3F0DF)

    // ------------------------------------------------------------- ink
    /// `--ink` #24331c — primary text, deep moss-black.
    public static let ink = Color(hex: 0x24331C)
    /// `--ink-soft` #4d5b3c — secondary text.
    public static let inkSoft = Color(hex: 0x4D5B3C)
    /// `--ink-faint` #6b7758 — tertiary / captions / mono labels.
    public static let inkFaint = Color(hex: 0x6B7758)

    // ------------------------------------------------------------- accents
    /// `--forest` #3c6b39 — primary brand green.
    public static let forest = Color(hex: 0x3C6B39)
    /// `--forest-deep` #2c5230 — pressed/hover green.
    public static let forestDeep = Color(hex: 0x2C5230)
    /// `--wood` #5f7d3d — secondary olive-green.
    public static let wood = Color(hex: 0x5F7D3D)
    /// `--amber` #e3ad33 — waypoint gold; highlights, progress, "you".
    public static let amber = Color(hex: 0xE3AD33)
    /// `--coral` #cf6a2a — trail-marker orange, emphasis.
    public static let coral = Color(hex: 0xCF6A2A)
    /// `--water` #33575b — deep teal counterpoint.
    public static let water = Color(hex: 0x33575B)
    /// `--warning` #c0492e — off-route / destructive.
    public static let warning = Color(hex: 0xC0492E)

    // ------------------------------------------------------------- lines
    /// `--line` rgba(47,82,51,.16) — hairline borders.
    public static let line = Color(hex: 0x2F5233).opacity(0.16)
    /// `--line-strong` rgba(47,82,51,.32).
    public static let lineStrong = Color(hex: 0x2F5233).opacity(0.32)
    /// `--scr-line` rgba(47,82,51,.14) — in-screen separators (list rows, nav).
    public static let screenLine = Color(hex: 0x2F5233).opacity(0.14)
    /// The faint parchment grid drawn behind tracks/profiles, rgba(47,82,51,.06).
    public static let gridLine = Color(hex: 0x2F5233).opacity(0.06)

    // ------------------------------------------------------------- iOS additions (§9)
    /// `--tint` = `--forest` — the iOS control tint.
    public static let tint = forest
    /// `--track-stroke` #d99a1f — deepened amber for a bold route stroke on parchment.
    public static let trackStroke = Color(hex: 0xD99A1F)
    /// `--track-halo` #f4ecc9 — light casing under the track stroke.
    public static let trackHalo = Color(hex: 0xF4ECC9)
    /// `--track-start` = `--forest` — start node dot.
    public static let trackStart = forest
    /// `--track-end` = `--coral` — end node dot.
    public static let trackEnd = coral

    // ------------------------------------------------------------- device illustration (§4)
    // The little hardware drawing on the launch/pairing screens. Pinned by the
    // design: `--dev-header` / `--dev-header-text` (tokens/colors.css) plus the
    // literals of the §4 frames — these draw the *device*, not app chrome.
    /// Device upper shell — the "Forest" colorway of the current industrial design.
    public static let deviceBody = Color(hex: 0x2F6350)
    /// Device lower shell — the "Celadon" base the body is seated on, showing as a lip.
    public static let deviceAccent = Color(hex: 0x8BE3BC)
    /// The four device buttons: dark moulded rubber.
    public static let deviceButton = Color(hex: 0x2B2F36)
    /// The deep black bezel the panel is recessed into.
    public static let deviceBezel = Color(hex: 0x101317)
    /// `--dev-header` #AA5500 — the device UI's rust title bar.
    public static let deviceHeader = Color(hex: 0xAA5500)
    /// `--dev-header-text` #FFFFAA — cream text on the title bar.
    public static let deviceHeaderText = Color(hex: 0xFFFFAA)
    /// The device screen's track amber (#FFAA00, the on-glass palette).
    public static let deviceTrack = Color(hex: 0xFFAA00)

    // ------------------------------------------------------------- chrome metrics (§9)
    /// iOS inline nav-bar height, 44pt.
    public static let navBarHeight: CGFloat = 44
    /// iOS status-bar band, 54pt.
    public static let statusBarHeight: CGFloat = 54
    /// iOS control corner radius (buttons/inputs), 13pt.
    public static let controlRadius: CGFloat = 13

    // ------------------------------------------------------------- radii (tokens/spacing.css)
    /// `--radius-sm` 7 — badges, pills, small controls (icon tiles).
    public static let radiusSmall: CGFloat = 7
    /// `--radius-md` 11 — inputs, segmented track, banners.
    public static let radiusMedium: CGFloat = 11
    /// 14 — grouped-list bodies, track/profile cards (the design's screen panels).
    public static let radiusPanel: CGFloat = 14
    /// `--radius-lg` 16 — route cards.
    public static let radiusCard: CGFloat = 16
    /// `--radius-xl` 20 — large feature panels.
    public static let radiusLarge: CGFloat = 20
    /// Bottom-sheet top corners, 22pt.
    public static let radiusSheet: CGFloat = 22

    // ------------------------------------------------------------- spacing (4px base)
    /// The 4-pt spacing scale: 4 / 8 / 12 / 16 / 22 / 30 / 46 / 64.
    public static let spacing: [CGFloat] = [4, 8, 12, 16, 22, 30, 46, 64]
}

extension Color {
    /// 24-bit sRGB literal, `0xRRGGBB` — how the token hex values are transcribed.
    init(hex: UInt32) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255
        )
    }
}
