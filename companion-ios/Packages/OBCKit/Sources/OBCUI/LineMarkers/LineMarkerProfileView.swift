import SwiftUI
import OBCDomain

/// The elevation profile with a real distance axis, filled per segment, with a draggable
/// handle on every marker. Each handle owns a 44 pt band: a drag in the band moves that
/// marker, a drag anywhere else scrolls the page.
struct LineMarkerProfileView: View {
    let model: LineMarkerEditorModel
    var height: CGFloat = 132

    private static let stopMarkSize: CGFloat = 14

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
                    drawStopTies(in: &context, plot: plot)
                }
                ForEach(Array(model.stops.enumerated()), id: \.offset) { _, placed in
                    StopIcon(kind: placed.stop.kind, size: Self.stopMarkSize, isRound: true)
                        .position(x: stopX(placed, plot: plot), y: Self.stopMarkSize / 2 + 4)
                        .allowsHitTesting(false)
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
            ProfileGrabBand(model: model, marker: marker, plot: plot)
                .position(x: x, y: plot.midY)
        }
    }

    // MARK: Drawing
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

    /// A stop mark's x, held inside the card.
    private func stopX(_ placed: PlacedStop, plot: CGRect) -> CGFloat {
        min(max(x(placed.distance, plot: plot), plot.minX + Self.stopMarkSize / 2), plot.maxX - Self.stopMarkSize / 2)
    }

    /// A faint dotted tie from each stop mark down to the floor.
    private func drawStopTies(in context: inout GraphicsContext, plot: CGRect) {
        for placed in model.stops {
            let x = stopX(placed, plot: plot)
            var tie = Path()
            tie.move(to: CGPoint(x: x, y: Self.stopMarkSize + 4))
            tie.addLine(to: CGPoint(x: x, y: plot.maxY))
            context.stroke(
                tie, with: .color(StopIcon.color(placed.stop.kind).opacity(0.25)),
                style: StrokeStyle(lineWidth: 1, dash: [2, 3]))
        }
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

/// The 44 pt band a finger grabs, one per marker, the full height of the plot so the finger
/// never has to find the pin. The band owns its gesture state, so a second finger on another
/// band is refused by the model and ends nothing.
private struct ProfileGrabBand: View {
    let model: LineMarkerEditorModel
    let marker: LineMarker
    let plot: CGRect

    /// Half the band: markers this close in x share the finger.
    private static let reach: CGFloat = 22

    /// The marker this band holds and its distance when the finger landed. `nil` until the
    /// first movement, which picks between coincident markers by its direction.
    @State private var grab: (id: LineMarker.ID, start: Double)?
    /// Resets on cancel as well as on end, unlike `onEnded`; the reset is what ends the drag.
    @GestureState private var isPressed = false

    var body: some View {
        Color.clear
            .frame(width: Self.reach * 2, height: plot.height)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .updating($isPressed) { _, pressed, _ in pressed = true }
                    .onChanged(dragChanged)
            )
            .onChange(of: isPressed) { _, pressed in
                if !pressed { release() }
            }
            .onDisappear(perform: release)
            .accessibilityElement()
            .accessibilityLabel(marker.name)
            .accessibilityValue("km \(OBCFormat.distanceValue(meters: marker.distance))")
            .accessibilityAdjustableAction { direction in
                let step = LineMarkerEditorModel.nudgeMeters
                model.nudge(marker.id, by: direction == .increment ? step : -step)
            }
    }

    private func dragChanged(_ value: DragGesture.Value) {
        if grab == nil {
            guard value.translation.width != 0 else { return }
            let metersPerPoint = model.line.length / Double(max(plot.width, 1))
            let reach = Double(Self.reach) * metersPerPoint
            let under = model.markers.filter { abs($0.distance - marker.distance) <= reach }.map(\.id)
            guard
                let id = model.grab(among: under, forward: value.translation.width > 0),
                let start = model.marker(id)?.distance,
                model.begin(id)
            else { return }
            grab = (id, start)
        }
        guard let grab else { return }
        // Delta from the grab, so the marker never jumps to the finger.
        let plotWidth = Double(max(plot.width, 1))
        model.move(grab.id, to: grab.start + Double(value.translation.width) / plotWidth * model.line.length)
    }

    private func release() {
        guard grab != nil else { return }
        grab = nil
        model.end()
    }
}
