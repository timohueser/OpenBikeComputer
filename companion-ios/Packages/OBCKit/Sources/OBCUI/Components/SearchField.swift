import SwiftUI

/// A sunken search bar with a leading magnifier and a trailing clear button once
/// there is a query. It filters the main-screen list. It is not a system
/// `.searchable`, so it can sit inside the custom chrome.
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
                .font(.system(.subheadline, weight: .semibold))
                .foregroundStyle(OBCTheme.secondary)

            TextField(prompt, text: $text)
                .font(.system(.subheadline))
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
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 12)
        .background(OBCTheme.fill)
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
            .background(OBCTheme.page)
        }
    }
    return Demo()
}
