import SwiftUI
import OBCDomain

/// The elevation profile of the model's window, filled per segment, with a draggable handle on
/// every marker. The fills and strokes are one static layer drawn for the markers at rest; a
/// drag moves only a light overlay (the re-tinted stretch behind the finger, the pins and the
/// label), so a frame never re-walks the profile. The window is set from outside; the profile
/// itself does not zoom or scroll.
struct LineMarkerProfileView: View {
    let model: LineMarkerEditorModel
    var height: CGFloat = 132
    /// Kilometre ticks under the plot, for a window that is not the whole line.
    var showsAxis = false

    /// Room above the curve for a handle and its label.
    private let inset = EdgeInsets(top: 52, leading: 14, bottom: 8, trailing: 14)

    var body: some View {
        VStack(spacing: 4) {
            GeometryReader { geometry in
                let plot = CGRect(
                    x: inset.leading, y: inset.top,
                    width: geometry.size.width - inset.leading - inset.trailing,
                    height: geometry.size.height - inset.top - inset.bottom
                )
                ZStack(alignment: .topLeading) {
                    let runs = model.runs(splits: model.restingMarkers.map(\.distance))
                    ProfileStaticLayer(
                        samples: model.profile, window: model.window, elevationRange: model.elevationRange,
                        splits: runs.splits, colors: runs.colors, dashed: runs.dashed, plot: plot,
                        trailing: inset.trailing
                    )
                    .equatable()
                    movedStretch(plot: plot)
                    ForEach(model.markers) { marker in
                        if model.window.contains(marker.distance) {
                            handle(marker, plot: plot)
                        }
                    }
                    if let id = model.activeID, let marker = model.marker(id) {
                        MarkerLabel(text: model.label(for: id))
                            .fixedSize()
                            .position(labelCenter(for: marker.distance, plot: plot))
                            .allowsHitTesting(false)
                    }
                }
            }
            .frame(height: height)
            .background(OBCTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            if showsAxis {
                ProfileAxis(window: model.window, leading: inset.leading, trailing: inset.trailing)
            }
        }
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
            .stroke(isActive ? OBCTheme.amber : OBCTheme.ink, lineWidth: isActive ? 2 : 1)
            .allowsHitTesting(false)
            MarkerHandleView(color: model.color(endingAt: marker.id), isActive: isActive, isFixed: marker.isFixed)
                .position(x: x, y: y - MarkerHandleView.size.height / 2)
                .allowsHitTesting(false)
            ProfileGrabBand(model: model, marker: marker, plot: plot, x: x)
        }
    }

    /// The stretch the drag in flight moved from one segment to the other, re-tinted; the rest
    /// of the fills stay in the static layer.
    @ViewBuilder
    private func movedStretch(plot: CGRect) -> some View {
        if let id = model.activeID, let index = model.markers.firstIndex(where: { $0.id == id }),
            index < model.restingMarkers.count {
            let rest = model.restingMarkers[index].distance
            let now = model.markers[index].distance
            let from = max(min(rest, now), model.window.lowerBound)
            let to = min(max(rest, now), model.window.upperBound)
            // Moved forward, the stretch joins the segment before the marker; backward, the one after.
            let color = model.segmentColors[now > rest ? index : index + 1]
            if to > from {
                Canvas { context, _ in
                    let points = curve(from: from, to: to, plot: plot)
                    var area = Path()
                    area.move(to: CGPoint(x: points[0].x, y: plot.maxY))
                    for point in points { area.addLine(to: point) }
                    area.addLine(to: CGPoint(x: points[points.count - 1].x, y: plot.maxY))
                    area.closeSubpath()
                    context.fill(area, with: .color(OBCTheme.surface))
                    context.fill(area, with: .color(color.opacity(0.22)))
                    var line = Path()
                    line.addLines(points)
                    context.stroke(line, with: .color(color), style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
                }
                .allowsHitTesting(false)
            }
        }
    }

    // MARK: Geometry

    private func x(_ distance: Double, plot: CGRect) -> CGFloat {
        let window = model.window
        return plot.minX + plot.width * CGFloat((distance - window.lowerBound) / max(window.upperBound - window.lowerBound, 1))
    }

    private func y(_ elevation: Double, plot: CGRect) -> CGFloat {
        let range = model.elevationRange
        let unit = (elevation - range.lowerBound) / (range.upperBound - range.lowerBound)
        return plot.maxY - plot.height * CGFloat(unit)
    }

    /// The curve between two distances: the resampled points inside, plus interpolated ends.
    private func curve(from: Double, to: Double, plot: CGRect) -> [CGPoint] {
        var points = [CGPoint(x: x(from, plot: plot), y: y(model.line.elevation(at: from), plot: plot))]
        for sample in model.profile where sample.distance > from && sample.distance < to {
            points.append(CGPoint(x: x(sample.distance, plot: plot), y: y(sample.elevation, plot: plot)))
        }
        points.append(CGPoint(x: x(to, plot: plot), y: y(model.line.elevation(at: to), plot: plot)))
        return points
    }

    /// Above the handle, as on the map, and held inside the card.
    private func labelCenter(for distance: Double, plot: CGRect) -> CGPoint {
        let handleTop = y(model.line.elevation(at: distance), plot: plot) - MarkerHandleView.size.height
        return CGPoint(
            x: min(max(x(distance, plot: plot), plot.minX + 50), plot.maxX - 50),
            y: max(handleTop - 8, 14)
        )
    }
}

/// The grid, and one area and one stroke per segment of the markers at rest, split by
/// interpolation so the colour changes exactly under a handle. Equatable, so a drag frame
/// leaves it untouched.
private struct ProfileStaticLayer: View, Equatable {
    let samples: [LineMarkerEditorModel.ProfileSample]
    let window: ClosedRange<Double>
    let elevationRange: ClosedRange<Double>
    let splits: [Double]
    let colors: [Color]
    let dashed: Set<Int>
    let plot: CGRect
    let trailing: CGFloat

    var body: some View {
        Canvas { context, _ in
            var grid = Path()
            var gridY = plot.maxY
            while gridY > plot.minY - 1 {
                grid.move(to: CGPoint(x: 0, y: gridY))
                grid.addLine(to: CGPoint(x: plot.maxX + trailing, y: gridY))
                gridY -= 24
            }
            context.stroke(grid, with: .color(OBCTheme.sketchLine), lineWidth: 1)

            // The samples cover the window alone; a segment past its end is cut at it.
            let bounds = [0] + splits + [window.upperBound]
            for segment in 0..<(bounds.count - 1) {
                let from = max(bounds[segment], window.lowerBound)
                let to = min(bounds[segment + 1], window.upperBound)
                guard to > from, segment < colors.count else { continue }
                var points = [CGPoint(x: x(from), y: y(elevation(at: from)))]
                for sample in samples where sample.distance > from && sample.distance < to {
                    points.append(CGPoint(x: x(sample.distance), y: y(sample.elevation)))
                }
                points.append(CGPoint(x: x(to), y: y(elevation(at: to))))

                let color = colors[segment]
                var area = Path()
                area.move(to: CGPoint(x: points[0].x, y: plot.maxY))
                for point in points { area.addLine(to: point) }
                area.addLine(to: CGPoint(x: points[points.count - 1].x, y: plot.maxY))
                area.closeSubpath()
                context.fill(area, with: .color(color.opacity(0.22)))

                var line = Path()
                line.addLines(points)
                let dash: [CGFloat] = dashed.contains(segment) ? [4, 6] : []
                context.stroke(line, with: .color(color), style: StrokeStyle(
                    lineWidth: 2.2, lineCap: .round, lineJoin: .round, dash: dash))
            }
        }
        .allowsHitTesting(false)
    }

    private func x(_ distance: Double) -> CGFloat {
        plot.minX + plot.width * CGFloat((distance - window.lowerBound) / max(window.upperBound - window.lowerBound, 1))
    }

    private func y(_ elevation: Double) -> CGFloat {
        let unit = (elevation - elevationRange.lowerBound) / (elevationRange.upperBound - elevationRange.lowerBound)
        return plot.maxY - plot.height * CGFloat(unit)
    }

    /// Linear between the two samples around `distance`; the samples are evenly spaced.
    private func elevation(at distance: Double) -> Double {
        guard samples.count > 1 else { return samples.first?.elevation ?? 0 }
        let origin = samples[0].distance
        let step = (samples[samples.count - 1].distance - origin) / Double(samples.count - 1)
        let position = min(max((distance - origin) / max(step, 1e-9), 0), Double(samples.count - 1))
        let i = min(Int(position), samples.count - 2)
        let t = position - Double(i)
        return samples[i].elevation + (samples[i + 1].elevation - samples[i].elevation) * t
    }
}

/// Kilometre ticks under the plot at a round step, about five across the window.
private struct ProfileAxis: View {
    let window: ClosedRange<Double>
    let leading: CGFloat
    let trailing: CGFloat

    private static let steps: [Double] = [0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500].map { $0 * 1000 }

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width - leading - trailing
            let span = max(window.upperBound - window.lowerBound, 1)
            let step = Self.steps.first { span / $0 <= 6 } ?? Self.steps[Self.steps.count - 1]
            let ticks = Array(stride(from: (window.lowerBound / step).rounded(.up) * step, through: window.upperBound, by: step))
            ForEach(ticks, id: \.self) { tick in
                Text(OBCFormat.distanceValue(meters: tick))
                    .font(.system(.caption2).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
                    .fixedSize()
                    .position(x: leading + width * CGFloat((tick - window.lowerBound) / span), y: geometry.size.height / 2)
            }
        }
        .frame(height: 14)
        .obcFixedGeometryType()
        .accessibilityHidden(true)
    }
}

/// The 44 pt band a finger grabs, one per marker, the full height of the plot so the finger
/// never has to find the pin. It stays where the finger landed for the whole drag, so the view
/// that owns the gesture never moves under the finger. A second finger on another band is
/// refused by the model and ends nothing.
private struct ProfileGrabBand: View {
    let model: LineMarkerEditorModel
    let marker: LineMarker
    let plot: CGRect
    /// The marker's x this frame.
    let x: CGFloat

    /// Half the band: markers this close in x share the finger.
    private static let reach: CGFloat = 22

    /// The marker this band holds, its distance and the band's x when the finger landed.
    /// `nil` until the first movement, which picks between coincident markers by its direction.
    @State private var grab: (id: LineMarker.ID, start: Double, x: CGFloat)?
    /// A first movement that went more up or down than sideways is the sheet's, not a drag of
    /// the marker: the gesture is left alone until the finger lifts.
    @State private var declined = false
    /// Resets on cancel as well as on end, unlike `onEnded`; the reset is what ends the drag.
    @GestureState private var isPressed = false

    var body: some View {
        Color.clear
            .frame(width: Self.reach * 2, height: plot.height)
            .contentShape(Rectangle())
            .position(x: grab?.x ?? x, y: plot.midY)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .updating($isPressed) { _, pressed, _ in pressed = true }
                    .onChanged(dragChanged)
            )
            .onChange(of: isPressed) { _, pressed in
                if !pressed { release() }
            }
            .onDisappear { if grab != nil { release() } }
            .accessibilityElement()
            .accessibilityIdentifier("profile.handle")
            .accessibilityLabel(marker.name)
            .accessibilityValue("km \(OBCFormat.distanceValue(meters: marker.distance))")
            .accessibilityAdjustableAction { direction in
                let step = LineMarkerEditorModel.nudgeMeters
                model.nudge(marker.id, by: direction == .increment ? step : -step)
            }
    }

    private func dragChanged(_ value: DragGesture.Value) {
        let window = model.window
        let metersPerPoint = (window.upperBound - window.lowerBound) / Double(max(plot.width, 1))
        if grab == nil {
            guard !declined, value.translation.width != 0 else { return }
            guard abs(value.translation.width) >= abs(value.translation.height) else {
                declined = true
                return
            }
            let reach = Double(Self.reach) * metersPerPoint
            let under = model.markers.filter { abs($0.distance - marker.distance) <= reach }.map(\.id)
            guard
                let id = model.grab(among: under, forward: value.translation.width > 0),
                let start = model.marker(id)?.distance,
                model.begin(id)
            else { return }
            grab = (id, start, x)
        }
        guard let grab else { return }
        // Delta from the grab, so the marker never jumps to the finger.
        model.move(grab.id, to: grab.start + Double(value.translation.width) * metersPerPoint)
    }

    private func release() {
        let wasDeclined = declined
        declined = false
        guard let grab else {
            if !wasDeclined { model.tap(marker.id) }
            return
        }
        self.grab = nil
        model.end()
    }
}
