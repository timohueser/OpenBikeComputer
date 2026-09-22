import SwiftUI
import OBCDomain

/// The elevation profile with a real distance axis, filled per segment, with a draggable
/// handle on every marker. Each handle owns a 44 pt band: a drag in the band moves that
/// marker, a drag anywhere else scrolls the page.
struct LineMarkerProfileView: View {
    let model: LineMarkerEditorModel
    var height: CGFloat = 132

    /// Room above the curve for a handle and its label.
    private let inset = EdgeInsets(top: 52, leading: 14, bottom: 8, trailing: 14)

    var body: some View {
        GeometryReader { geometry in
            let plot = CGRect(
                x: inset.leading, y: inset.top,
                width: geometry.size.width - inset.leading - inset.trailing,
                height: geometry.size.height - inset.top - inset.bottom
            )
            ZStack(alignment: .topLeading) {
                Canvas { context, _ in
                    drawGrid(in: &context, plot: plot)
                    drawSegments(in: &context, plot: plot)
                }
                ForEach(model.markers) { marker in
                    handle(marker, plot: plot)
                }
                if let id = model.activeID, let marker = model.marker(id) {
                    MarkerLabel(text: model.label(for: id))
                        .fixedSize()
                        .position(labelCenter(for: marker, plot: plot))
                        .allowsHitTesting(false)
                }
            }
        }
        .frame(height: height)
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
    }

    // MARK: Handles

    private func handle(_ marker: LineMarker, plot: CGRect) -> some View {
        let x = x(marker.distance, plot: plot)
        let y = y(model.line.elevation(at: marker.distance), plot: plot)
        let isActive = model.activeID == marker.id
        return ZStack {
            // From the floor up to the handle, which caps it; the label above stays clear.
            Path { path in
                path.move(to: CGPoint(x: x, y: y))
                path.addLine(to: CGPoint(x: x, y: plot.maxY))
            }
            .stroke(isActive ? OBCTheme.forest : OBCTheme.ink, lineWidth: isActive ? 1.5 : 1)
            .allowsHitTesting(false)
            MarkerHandleView(color: model.color(endingAt: marker.id), isActive: isActive)
                .position(x: x, y: y - MarkerHandleView.size.height / 2)
                .allowsHitTesting(false)
            // The grab band: the full height, so the finger never has to find the knob.
            Color.clear
                .frame(width: 44, height: plot.height)
                .contentShape(Rectangle())
                .position(x: x, y: plot.midY)
                .gesture(drag(marker, plotWidth: plot.width))
                .accessibilityElement()
                .accessibilityLabel(marker.name)
                .accessibilityValue("km \(OBCFormat.distanceValue(meters: marker.distance))")
                .accessibilityAdjustableAction { direction in
                    let step = LineMarkerEditorModel.nudgeMeters
                    model.nudge(marker.id, by: direction == .increment ? step : -step)
                }
        }
    }

    private func drag(_ marker: LineMarker, plotWidth: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if model.activeID != marker.id {
                    model.begin(marker.id)
                }
                // Delta from the grab, so the marker never jumps to the finger.
                let start = grabStart ?? marker.distance
                if grabStart == nil { grabStart = start }
                model.move(marker.id, to: start + Double(value.translation.width / max(plotWidth, 1)) * model.line.length)
            }
            .onEnded { _ in
                model.end()
                grabStart = nil
            }
    }

    /// The marker's distance when the finger landed.
    @State private var grabStart: Double?

    // MARK: Drawing

    private func x(_ distance: Double, plot: CGRect) -> CGFloat {
        plot.minX + plot.width * CGFloat(distance / max(model.line.length, 1))
    }

    private func y(_ elevation: Double, plot: CGRect) -> CGFloat {
        let range = model.elevationRange
        let unit = (elevation - range.lowerBound) / (range.upperBound - range.lowerBound)
        return plot.maxY - plot.height * CGFloat(unit)
    }

    /// Above the handle, as on the map, and held inside the card.
    private func labelCenter(for marker: LineMarker, plot: CGRect) -> CGPoint {
        let handleTop = y(model.line.elevation(at: marker.distance), plot: plot) - MarkerHandleView.size.height
        return CGPoint(
            x: min(max(x(marker.distance, plot: plot), plot.minX + 40), plot.maxX - 40),
            y: max(handleTop - 8, 14)
        )
    }

    private func drawGrid(in context: inout GraphicsContext, plot: CGRect) {
        var grid = Path()
        var y = plot.maxY
        while y > plot.minY - 1 {
            grid.move(to: CGPoint(x: 0, y: y))
            grid.addLine(to: CGPoint(x: plot.maxX + inset.trailing, y: y))
            y -= 24
        }
        context.stroke(grid, with: .color(OBCTheme.gridLine), lineWidth: 1)
    }

    /// One area and one stroke per segment, split at the markers by interpolation so the
    /// colour changes exactly under the handle.
    private func drawSegments(in context: inout GraphicsContext, plot: CGRect) {
        let bounds = [0] + model.markers.map(\.distance) + [model.line.length]
        for segment in 0..<(bounds.count - 1) {
            let from = bounds[segment], to = bounds[segment + 1]
            guard to > from else { continue }
            var points = [CGPoint(x: x(from, plot: plot), y: y(model.line.elevation(at: from), plot: plot))]
            for sample in model.profile where sample.distance > from && sample.distance < to {
                points.append(CGPoint(x: x(sample.distance, plot: plot), y: y(sample.elevation, plot: plot)))
            }
            points.append(CGPoint(x: x(to, plot: plot), y: y(model.line.elevation(at: to), plot: plot)))

            let color = model.segmentColors[segment]
            var area = Path()
            area.move(to: CGPoint(x: points[0].x, y: plot.maxY))
            for point in points { area.addLine(to: point) }
            area.addLine(to: CGPoint(x: points[points.count - 1].x, y: plot.maxY))
            area.closeSubpath()
            context.fill(area, with: .color(color.opacity(0.22)))

            var line = Path()
            line.addLines(points)
            context.stroke(line, with: .color(color), style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
        }
    }
}
