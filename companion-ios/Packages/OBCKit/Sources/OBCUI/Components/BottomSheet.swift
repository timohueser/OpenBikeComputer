import SwiftUI

/// Grabber and panel chrome for the sheets that host upload progress and
/// confirmations without leaving the route. Use it inside a `.sheet` presentation, and
/// pair it with a `.presentationDetents` height sized to the content.
public struct OBCSheetContainer<Content: View>: View {
    @ViewBuilder let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        VStack(spacing: 0) {
            // Our own grabber: the presentation drag indicator stays hidden so there
            // is exactly one.
            RoundedRectangle(cornerRadius: 3)
                .fill(OBCTheme.ink.opacity(0.22))
                .frame(width: 38, height: 5)
                .padding(.top, 12)
                .padding(.bottom, 16)

            content
                .padding(.horizontal, 22)
                .padding(.bottom, 40)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(OBCTheme.radiusSheet)
        .presentationBackground(OBCTheme.panel)
    }
}

#Preview("Bottom sheet") {
    struct Demo: View {
        @State private var shown = true
        var body: some View {
            OBCTheme.parchment
                .ignoresSafeArea()
                .sheet(isPresented: $shown) {
                    OBCSheetContainer {
                        VStack(alignment: .leading, spacing: 14) {
                            Text("Uploading to Trailhead")
                                .font(.obcSerif(size: 22))
                                .foregroundStyle(OBCTheme.ink)
                            OBCProgressBar(value: 0.62)
                            Text("2.1 MB of 3.4 MB")
                                .font(.obcMono(size: 12))
                                .foregroundStyle(OBCTheme.inkFaint)
                            Button("Cancel") {}.buttonStyle(.obcGhost)
                        }
                    }
                    .presentationDetents([.height(260)])
                }
        }
    }
    return Demo()
}
