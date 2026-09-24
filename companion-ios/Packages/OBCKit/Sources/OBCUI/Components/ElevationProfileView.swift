import SwiftUI
import OBCDomain

/// An elevation area and line over a faint grid, in a panel card, with high and low
/// markers. Renders plain elevation samples, so any source feeds it after a cheap
/// extraction.
public struct ElevationProfileView: View {
    /// Elevation samples in metres, assumed evenly spaced along the route.
    let samples: [Double]
    var height: CGFloat
    /// False draws the bare profile, for a caller that frames it itself.
    var card: Bool
    /// Photo ticks on the floor, each from 0 at the start to 1 at the end.
    var ticks: [Double]

    public init(samples: [Double], height: CGFloat = 80, card: Bool = true, ticks: [Double] = []) {
        self.samples = samples
        self.height = height
        self.card = card
        self.ticks = ticks
    }

    /// From an imported route's points (skips missing elevations).
    public init(routePoints: [RoutePoint], height: CGFloat = 80) {
        self.init(samples: routePoints.compactMap(\.elevationMeters), height: height)
    }

    @ViewBuilder
    public var body: some View {
        if card {
            profile
                .padding(.top, 16)
                .padding(.horizontal, 12)
                .padding(.bottom, 10)
                .background(OBCTheme.surface)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                .accessibilityLabel("Elevation profile")
                // The card only exists once its samples do; on a tracked ride that is an async
                // read away. Automation waits on this to know the layout is final.
                .accessibilityIdentifier("detail.elevationProfile")
        } else {
            profile
        }
    }

    private var profile: some View {
        Canvas { context, size in
            var grid = Path()
            var y: CGFloat = 0
            while y < size.height {
                grid.move(to: CGPoint(x: 0, y: y))
                grid.addLine(to: CGPoint(x: size.width, y: y))
                y += 24
            }
            context.stroke(grid, with: .color(OBCTheme.sketchLine), lineWidth: 1)

            guard samples.count > 1 else { return }
            let lo = samples.min()!
            let hi = samples.max()!
            let span = max(hi - lo, 1)
            // Headroom keeps the line and its markers inside the card.
            let top = size.height * 0.15
            let bottom = size.height - 4
            // The end markers' dots stay whole inside the canvas.
            let inset: CGFloat = 4.5
            let points = samples.enumerated().map { index, sample in
                CGPoint(
                    x: inset + (size.width - 2 * inset) * CGFloat(index) / CGFloat(samples.count - 1),
                    y: bottom - (bottom - top) * CGFloat((sample - lo) / span)
                )
            }

            // The fill hangs from the curve to the card's floor, built by hand:
            // `addLines` opens a new subpath at its first point, so a prior corner
            // `move` is orphaned and `close` would draw a stray diagonal.
            var area = Path()
            area.move(to: CGPoint(x: points[0].x, y: size.height))
            for point in points { area.addLine(to: point) }
            area.addLine(to: CGPoint(x: points[points.count - 1].x, y: size.height))
            area.closeSubpath()
            context.fill(area, with: .color(OBCTheme.profileFill))

            var line = Path()
            line.addLines(points)
            context.stroke(
                line,
                with: .color(OBCTheme.amber),
                style: StrokeStyle(lineWidth: 2.4, lineCap: .round, lineJoin: .round)
            )

            drawExtremeMarkers(in: &context, points: points, size: size)

            for tick in ticks {
                let x = inset + (size.width - 2 * inset) * CGFloat(min(max(tick, 0), 1))
                context.fill(
                    Path(roundedRect: CGRect(x: x - 1, y: size.height - 8, width: 2, height: 8), cornerRadius: 1),
                    with: .color(OBCTheme.secondary)
                )
            }
        }
        .frame(height: height)
    }

    /// Dots and labels on the highest and lowest samples. The high label tucks
    /// below its dot and the low label above, so both always have room.
    private func drawExtremeMarkers(in context: inout GraphicsContext, points: [CGPoint], size: CGSize) {
        guard
            points.count == samples.count,
            let hi = samples.max(), let lo = samples.min(), hi > lo,
            let maxIndex = samples.firstIndex(of: hi),
            let minIndex = samples.firstIndex(of: lo)
        else { return }
        drawMarker(in: &context, at: points[maxIndex], meters: hi, labelOffset: 10, size: size)
        drawMarker(in: &context, at: points[minIndex], meters: lo, labelOffset: -10, size: size)
    }

    private func drawMarker(
        in context: inout GraphicsContext, at point: CGPoint,
        meters: Double, labelOffset: CGFloat, size: CGSize
    ) {
        let dot = CGRect(x: point.x - 3, y: point.y - 3, width: 6, height: 6)
        context.fill(Path(ellipseIn: dot.insetBy(dx: -1.5, dy: -1.5)), with: .color(OBCTheme.surface))
        context.fill(Path(ellipseIn: dot), with: .color(OBCTheme.amber))

        let label = context.resolve(
            Text("\(Int(meters.rounded())) m")
                .font(.system(.caption2, weight: .semibold).monospacedDigit())
                .foregroundColor(OBCTheme.secondary)
        )
        let halfWidth = label.measure(in: size).width / 2
        let x = min(max(point.x, halfWidth), size.width - halfWidth)
        context.draw(
            label,
            at: CGPoint(x: x, y: point.y + labelOffset),
            anchor: labelOffset > 0 ? .top : .bottom
        )
    }
}

#Preview("Elevation profile") {
    VStack(alignment: .leading, spacing: 4) {
        OBCEyebrow("Elevation profile")
        ElevationProfileView(samples: [220, 260, 240, 380, 330, 470, 360, 450, 390, 410])
    }
    .padding(20)
    .background(OBCTheme.page)
}
