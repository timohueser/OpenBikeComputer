import SwiftUI

/// **Bottom Sheet** (§9, NEW) — grabber + panel chrome for the sheets that host
/// upload progress and confirmations without leaving the route (U1/U2, H1).
///
/// Use inside a `.sheet` presentation; pair with `.presentationDetents` sized
/// to the content:
///
///     .sheet(isPresented: $showUpload) {
///         OBCSheetContainer { UploadProgressContent() }
///             .presentationDetents([.height(280)])
///     }
public struct OBCSheetContainer<Content: View>: View {
    @ViewBuilder let content: Content

    public init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    public var body: some View {
        VStack(spacing: 0) {
            // The design's own grabber (38×5, ink 22%) — presentation drag
            // indicator stays hidden so there's exactly one.
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
