import SwiftUI
import OBCDomain

/// **Elevation Profile** (§9, NEW) — area + line over a faint horizontal grid,
/// in a panel card (14pt radius; grid every 24pt; 2.4pt `trackStroke` line over
/// an 18%-alpha area fill). Renders plain elevation samples so any source
/// (imported route, downloaded ride) feeds it after a cheap extraction.
public struct ElevationProfileView: View {
    /// Elevation samples in metres, assumed evenly spaced along the route.
    let samples: [Double]
    var height: CGFloat

    public init(samples: [Double], height: CGFloat = 80) {
        self.samples = samples
        self.height = height
    }

    /// From an imported route's points (skips missing elevations).
    public init(routePoints: [RoutePoint], height: CGFloat = 80) {
        self.init(samples: routePoints.compactMap(\.elevationMeters), height: height)
    }

    /// From a ride's tracklog (skips missing elevations).
    public init(ridePoints: [RidePoint], height: CGFloat = 80) {
        self.init(samples: ridePoints.compactMap(\.elevationMeters), height: height)
    }

    public var body: some View {
        Canvas { context, size in
            // Horizontal gridlines every 24pt, edge to edge.
            var grid = Path()
            var y: CGFloat = 0
            while y < size.height {
                grid.move(to: CGPoint(x: 0, y: y))
                grid.addLine(to: CGPoint(x: size.width, y: y))
                y += 24
            }
            context.stroke(grid, with: .color(OBCTheme.gridLine), lineWidth: 1)

            guard samples.count > 1 else { return }
            let lo = samples.min()!
            let hi = samples.max()!
            let span = max(hi - lo, 1)
            // Keep the line inside the card with a little headroom, like the
            // design SVGs (top ~18%, bottom ~2pt above the baseline).
            let top = size.height * 0.15
            let bottom = size.height - 4
            let points = samples.enumerated().map { index, sample in
                CGPoint(
                    x: size.width * CGFloat(index) / CGFloat(samples.count - 1),
                    y: bottom - (bottom - top) * CGFloat((sample - lo) / span)
                )
            }

            var area = Path()
            area.move(to: CGPoint(x: 0, y: size.height))
            area.addLines(points)
            area.addLine(to: CGPoint(x: size.width, y: size.height))
            area.closeSubpath()
            context.fill(area, with: .color(OBCTheme.trackStroke.opacity(0.18)))

            var line = Path()
            line.addLines(points)
            context.stroke(
                line,
                with: .color(OBCTheme.trackStroke),
                style: StrokeStyle(lineWidth: 2.4, lineCap: .round, lineJoin: .round)
            )
        }
        .frame(height: height)
        .padding(.top, 14)
        .padding(.horizontal, 12)
        .padding(.bottom, 10)
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
        .accessibilityLabel("Elevation profile")
    }
}

#Preview("Elevation profile") {
    VStack(alignment: .leading, spacing: 4) {
        OBCEyebrow("Elevation profile")
        ElevationProfileView(samples: [220, 260, 240, 380, 330, 470, 360, 450, 390, 410])
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
