import SwiftUI

/// The rider's choice of Day or Tent colours: Automatic follows iOS. One `UserDefaults` key, read
/// at the app root, so every screen, sheet and map follows it.
public enum OBCAppearance: String, CaseIterable, Sendable {
    case automatic, light, dark

    public static let storageKey = "obc.appearance"

    public var name: String {
        switch self {
        case .automatic: "Automatic"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    /// Nil lets iOS decide.
    public var colorScheme: ColorScheme? {
        switch self {
        case .automatic: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

public extension View {
    /// Applies the stored appearance. Use once, at the app root.
    func obcAppearance() -> some View {
        modifier(AppearanceModifier())
    }
}

private struct AppearanceModifier: ViewModifier {
    @AppStorage(OBCAppearance.storageKey) private var appearance = OBCAppearance.automatic

    func body(content: Content) -> some View {
        content.preferredColorScheme(appearance.colorScheme)
    }
}

/// The Appearance row: the current choice, and a menu of the three.
struct OBCAppearanceRow: View {
    @AppStorage(OBCAppearance.storageKey) private var appearance = OBCAppearance.automatic

    var body: some View {
        Menu {
            Picker("Appearance", selection: $appearance) {
                ForEach(OBCAppearance.allCases, id: \.self) { Text($0.name).tag($0) }
            }
        } label: {
            OBCListRow(icon: "circle.lefthalf.filled", iconColor: OBCTheme.tint, label: "Appearance",
                       value: appearance.name, showsDivider: false) {
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(.footnote, weight: .semibold))
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("settings.appearance")
    }
}
