import SwiftUI

/// Grabber and panel chrome for the sheets that host upload progress and
/// confirmations without leaving the route. Use it inside a `.sheet` presentation. It sizes
/// its own detent to the content, so the last control sits as far from the sheet's bottom edge
/// as the content sits from its sides; content taller than the screen scrolls.
public struct OBCSheetContainer<Content: View>: View {
    @ViewBuilder let content: Content
    @State private var contentHeight: CGFloat = 0
    @State private var bottomInset: CGFloat = 0

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    /// The grabber and its spacing above the content.
    private static var grabberBand: CGFloat { 12 + 5 + 16 }

    public var body: some View {
        VStack(spacing: 0) {
            // Our own grabber: the presentation drag indicator stays hidden so there
            // is exactly one.
            RoundedRectangle(cornerRadius: 3)
                .fill(OBCTheme.ink.opacity(0.22))
                .frame(width: 38, height: 5)
                .padding(.top, 12)
                .padding(.bottom, 16)

            ScrollView {
                content
                    .padding(.horizontal, 22)
                    .padding(.bottom, 22)
                    .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { contentHeight = $0 }
            }
            .scrollBounceBehavior(.basedOnSize)
            .ignoresSafeArea(.container, edges: .bottom)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.bottom } action: { bottomInset = $0 }
        // A detent height stops at the bottom safe area, and the sheet runs on under the home
        // indicator, so the content reaches into that band instead of stacking above it.
        .presentationDetents([.height(Self.grabberBand + max(contentHeight, 120) - bottomInset)])
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(OBCTheme.radiusSheet)
        .presentationBackground(OBCTheme.surface)
    }
}

#Preview("Bottom sheet") {
    struct Demo: View {
        @State private var shown = true
        var body: some View {
            OBCTheme.page
                .ignoresSafeArea()
                .sheet(isPresented: $shown) {
                    OBCSheetContainer {
                        VStack(alignment: .leading, spacing: 14) {
                            Text("Uploading to Trailhead")
                                .font(.system(.title2, weight: .bold))
                                .foregroundStyle(OBCTheme.ink)
                            OBCProgressBar(value: 0.62)
                            Text("2.1 MB of 3.4 MB")
                                .font(.system(.caption).monospacedDigit())
                                .foregroundStyle(OBCTheme.secondary)
                            Button("Cancel") {}.buttonStyle(.obcGhost)
                        }
                    }
                }
        }
    }
    return Demo()
}
