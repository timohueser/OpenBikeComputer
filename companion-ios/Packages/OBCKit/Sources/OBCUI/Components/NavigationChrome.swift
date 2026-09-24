import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// Native chrome. `OBCNavigationChrome.apply()` recolours `UINavigationBar` globally and keeps
/// its system fonts, which follow Dynamic Type; `OBCLargeTitleBar` is the custom large-title row for
/// screens that pair the title with trailing circular actions, where the system bar
/// cannot host the device top bar above it.
public enum OBCNavigationChrome {
    /// Restyle the system navigation bar. UIKit appearance: call it once at app start.
    @MainActor
    public static func apply() {
        #if canImport(UIKit)
        let appearance = UINavigationBarAppearance()
        appearance.configureWithOpaqueBackground()
        appearance.backgroundColor = UIColor(OBCTheme.page)
        appearance.shadowColor = nil
        appearance.largeTitleTextAttributes = [.foregroundColor: UIColor(OBCTheme.ink)]
        appearance.titleTextAttributes = [.foregroundColor: UIColor(OBCTheme.ink)]

        UINavigationBar.appearance().standardAppearance = appearance
        UINavigationBar.appearance().scrollEdgeAppearance = appearance
        UINavigationBar.appearance().compactAppearance = appearance
        UINavigationBar.appearance().tintColor = UIColor(OBCTheme.tint)
        #endif
    }
}

/// The large-title row: the title with trailing circular actions, bottom-aligned.
public struct OBCLargeTitleBar<Actions: View>: View {
    let title: String
    @ViewBuilder let actions: Actions

    public init(_ title: String, @ViewBuilder actions: () -> Actions = { EmptyView() }) {
        self.title = title
        self.actions = actions()
    }

    public var body: some View {
        HStack(alignment: .bottom, spacing: 12) {
            Text(title)
                .font(.system(.largeTitle, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
            Spacer(minLength: 0)
            HStack(spacing: 8) { actions }
                .padding(.bottom, 3)
        }
        .padding(.top, 6)
        .padding(.horizontal, 20)
        .padding(.bottom, 12)
    }
}

#Preview("Large title bar") {
    VStack(spacing: 0) {
        OBCLargeTitleBar("Routes") {
            OBCImportButton(fileExtensions: ["gpx", "tcx"]) { _ in }
        }
        Spacer()
    }
    .background(OBCTheme.page)
}
