import SwiftUI

/// Shimmering placeholder blocks: skeletons, not spinners. `OBCSkeleton` is
/// the raw shimmer block; `TrackRowSkeleton` is shaped like a list row.
/// Cached content appears instantly; only a fresh read shimmers. With Reduce Motion the block
/// holds still.
public struct OBCSkeleton: View {
    var cornerRadius: CGFloat = 8

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(cornerRadius: CGFloat = 8) {
        self.cornerRadius = cornerRadius
    }

    public var body: some View {
        if reduceMotion {
            OBCTheme.fill.clipShape(RoundedRectangle(cornerRadius: cornerRadius))
        } else {
            shimmer
        }
    }

    private var shimmer: some View {
        TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let phase = context.date.timeIntervalSinceReferenceDate
                .truncatingRemainder(dividingBy: 1.4) / 1.4
            GeometryReader { geo in
                OBCTheme.fill
                    .overlay {
                        LinearGradient(
                            colors: [.clear, OBCTheme.surface.opacity(0.55), .clear],
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

#Preview("Skeletons") {
    VStack(spacing: 12) {
        TrackRowSkeleton()
        TrackRowSkeleton()
        TrackRowSkeleton()
    }
    .padding(20)
    .background(OBCTheme.page)
}
