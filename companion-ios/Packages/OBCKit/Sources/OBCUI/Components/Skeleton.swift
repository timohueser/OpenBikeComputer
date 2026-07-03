import SwiftUI

/// Skeleton loader — shimmering parchment placeholder blocks ("skeletons, not
/// spinners"). `OBCSkeleton` is the raw shimmer block; `RouteCardSkeleton` is
/// a placeholder shaped like a compact route card. Cached content appears
/// instantly; only the fresh read shimmers.
public struct OBCSkeleton: View {
    var cornerRadius: CGFloat = 8

    public init(cornerRadius: CGFloat = 8) {
        self.cornerRadius = cornerRadius
    }

    public var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let phase = context.date.timeIntervalSinceReferenceDate
                .truncatingRemainder(dividingBy: 1.4) / 1.4
            GeometryReader { geo in
                OBCTheme.parchment3
                    .overlay {
                        LinearGradient(
                            colors: [.clear, .white.opacity(0.55), .clear],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                        .frame(width: geo.size.width)
                        // Sweep from fully off-screen left to off-screen right.
                        .offset(x: (2 * phase - 1) * geo.size.width * 1.5)
                    }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
    }
}

/// A compact route card's shape while it loads: the 128pt track block plus a
/// title and stat-line bar.
public struct RouteCardSkeleton: View {
    public init() {}

    public var body: some View {
        HStack(spacing: 0) {
            OBCSkeleton(cornerRadius: 0)
                .frame(width: 128)

            VStack(alignment: .leading, spacing: 9) {
                OBCSkeleton().frame(height: 15).frame(maxWidth: .infinity, alignment: .leading)
                    .containerRelativeFrame(.horizontal) { length, _ in length * 0.45 }
                OBCSkeleton().frame(height: 11)
                    .containerRelativeFrame(.horizontal) { length, _ in length * 0.32 }
            }
            .padding(.vertical, 13)
            .padding(.horizontal, 15)
            .frame(maxWidth: .infinity, minHeight: 96, alignment: .leading)
        }
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusCard).strokeBorder(OBCTheme.line))
        .accessibilityLabel("Loading")
    }
}

#Preview("Skeletons") {
    VStack(spacing: 12) {
        RouteCardSkeleton()
        RouteCardSkeleton()
        RouteCardSkeleton()
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
