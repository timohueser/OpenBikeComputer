import SwiftUI

/// **Search Field** (§9, EXT) — the design's `.search` row: a sunken
/// `parchment-3` bar (11pt radius) with a leading magnifier and a trailing ✕
/// once there's a query (H6 keeps the query editable). Filters the main-screen
/// list; not a system `.searchable` so it can sit inside the custom chrome.
public struct OBCSearchField: View {
    @Binding var text: String
    let prompt: String

    public init(text: Binding<String>, prompt: String = "Search routes") {
        self._text = text
        self.prompt = prompt
    }

    public var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(OBCTheme.inkFaint)

            TextField(prompt, text: $text)
                .font(.system(size: 15))
                .foregroundStyle(OBCTheme.ink)
                .autocorrectionDisabled()
                .submitLabel(.search)
                #if os(iOS)
                .textInputAutocapitalization(.never)
                #endif

            if !text.isEmpty {
                Button {
                    text = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 15))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 12)
        .background(OBCTheme.parchment3)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
    }
}

#Preview("Search field") {
    struct Demo: View {
        @State private var empty = ""
        @State private var query = "devils"
        var body: some View {
            VStack(spacing: 14) {
                OBCSearchField(text: $empty)
                OBCSearchField(text: $query)
            }
            .padding(20)
            .background(OBCTheme.parchment)
        }
    }
    return Demo()
}
