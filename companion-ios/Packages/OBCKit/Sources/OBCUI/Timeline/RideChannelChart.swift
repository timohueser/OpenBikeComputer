import SwiftUI
import OBCDomain

extension RideTimeline.Channel {
    var title: String {
        switch self {
        case .elevation: "Elevation"
        case .speed: "Speed"
        case .heartRate: "Heart rate"
        case .power: "Power"
        case .cadence: "Cadence"
        }
    }

    /// The timeline's short strip label.
    var stripLabel: String {
        switch self {
        case .elevation: "Elev"
        case .speed: "Speed"
        case .heartRate: "HR"
        case .power: "Power"
        case .cadence: "Cadence"
        }
    }

    var unit: String {
        switch self {
        case .elevation: "m"
        case .speed: "kph"
        case .heartRate: "bpm"
        case .power: "W"
        case .cadence: "rpm"
        }
    }

    /// The value without its unit; speed comes in metres per second.
    func format(_ value: Double) -> String {
        switch self {
        case .elevation: OBCFormat.climbValue(meters: value)
        case .speed: OBCFormat.speedValue(mps: value)
        case .heartRate, .power, .cadence: "\(Int(value.rounded()))"
        }
    }

    var stripHeight: CGFloat {
        switch self {
        case .elevation: 58
        case .speed: 44
        case .heartRate, .power: 50
        case .cadence: 34
        }
    }
}

extension RideTimeline {
    /// The value span a chart draws: the plotted values, widened so a zoned channel shows its
    /// zones in proportion and a flat line does not magnify noise.
    func chartRange(_ channel: Channel, plot: Plot) -> ClosedRange<Double>? {
        guard let range = plot.range else { return nil }
        let (low, high) = (range.lowerBound, range.upperBound)
        switch channel {
        case .elevation:
            return low...max(high, low + 50)
        case .speed, .cadence:
            return 0...max(high, 1)
        case .heartRate:
            guard let maxHR = limits.maxHeartRate, maxHR > 0 else { return (low - 5)...(high + 5) }
            return min(low, Double(maxHR) * 0.5)...max(high, Double(maxHR) * 0.95)
        case .power:
            return 0...max(high, Double(limits.ftpWatts ?? 0) * 1.1, 1)
        }
    }

    /// The colour of a value on the timeline: its zone, else plain.
    func color(_ channel: Channel, value: Double) -> Color {
        zone(channel, value: value).map { OBCTheme.zones[$0] } ?? OBCTheme.ink
    }
}

/// One channel's plot and the cursor through it. The owner computes the plot for the chart's
/// width, so a redraw on a cursor move never touches the ride's samples.
struct RideChannelChart: View {
    enum Style {
        /// A timeline strip: heart rate and power coloured by zone, a plain elevation line.
        case strip
        /// The detail chart: zone bands behind heart rate and power, elevation coloured by grade.
        case detail
    }

    let timeline: RideTimeline
    let channel: RideTimeline.Channel
    let plot: RideTimeline.Plot
    var grades: [Double] = []
    /// Marks on the floor, each from 0 at the start to 1 at the end.
    var ticks: [Double] = []
    let style: Style
    let cursor: Double?

    var body: some View {
        Canvas { context, size in
            guard let range = timeline.chartRange(channel, plot: plot), plot.values.count > 1 else { return }
            let columns = plot.values.count
            let inset: CGFloat = style == .detail ? 6 : 3
            let span = max(range.upperBound - range.lowerBound, 1)
            func y(_ value: Double) -> CGFloat {
                size.height - inset - (size.height - 2 * inset) * CGFloat((value - range.lowerBound) / span)
            }
            func point(_ column: Int, _ value: Double) -> CGPoint {
                CGPoint(x: size.width * (CGFloat(column) + 0.5) / CGFloat(columns), y: y(value))
            }

            if style == .detail, let edges = timeline.zoneEdges(channel) {
                let bounds = [range.lowerBound] + edges + [range.upperBound]
                for zone in 0..<RideTimeline.zoneCount {
                    let top = y(min(max(bounds[zone + 1], range.lowerBound), range.upperBound))
                    let bottom = y(min(max(bounds[zone], range.lowerBound), range.upperBound))
                    guard bottom > top else { continue }
                    context.fill(Path(CGRect(x: 0, y: top, width: size.width, height: bottom - top)),
                                 with: .color(OBCTheme.zones[zone].opacity(0.16)))
                }
            }

            if channel == .elevation {
                // Each run of known values hangs its own fill to the floor.
                var area = Path()
                var lastX: CGFloat?
                for column in 0...columns {
                    guard column < columns, let value = plot.values[column] else {
                        if let x = lastX { area.addLine(to: CGPoint(x: x, y: size.height)) }
                        lastX = nil
                        continue
                    }
                    let p = point(column, value)
                    if lastX == nil { area.move(to: CGPoint(x: p.x, y: size.height)) }
                    area.addLine(to: p)
                    lastX = p.x
                }
                context.fill(area, with: .color(OBCTheme.profileFill))
            }

            // One path per colour, so a run of one zone strokes as one line.
            var paths: [Int: Path] = [:]
            var current: Int?
            for column in 1..<columns {
                guard let a = plot.values[column - 1], let b = plot.values[column] else {
                    current = nil
                    continue
                }
                let key = colorKey(column: column, value: b)
                if key != current {
                    paths[key, default: Path()].move(to: point(column - 1, a))
                    current = key
                }
                paths[key]?.addLine(to: point(column, b))
            }
            let width: CGFloat = channel == .cadence ? 1.6 : (style == .detail ? 2.4 : 2.1)
            for (key, path) in paths {
                context.stroke(path, with: .color(color(key)),
                               style: StrokeStyle(lineWidth: width, lineCap: .round, lineJoin: .round))
            }

            for tick in ticks {
                let x = size.width * CGFloat(min(max(tick, 0), 1))
                context.fill(Path(roundedRect: CGRect(x: x - 1, y: size.height - 7, width: 2, height: 7), cornerRadius: 1),
                             with: .color(OBCTheme.secondary))
            }

            if let cursor, timeline.length > 0 {
                let column = timeline.column(at: cursor, columns: columns)
                let x = size.width * (CGFloat(column) + 0.5) / CGFloat(columns)
                context.stroke(Path { $0.move(to: CGPoint(x: x, y: 0)); $0.addLine(to: CGPoint(x: x, y: size.height)) },
                               with: .color(OBCTheme.ink.opacity(0.55)), lineWidth: 1)
                if style == .detail, let value = plot.values[column] {
                    let dot = CGRect(x: x - 4.5, y: y(value) - 4.5, width: 9, height: 9)
                    context.fill(Path(ellipseIn: dot), with: .color(OBCTheme.surface))
                    context.stroke(Path(ellipseIn: dot), with: .color(OBCTheme.ink), lineWidth: 2)
                }
            }
        }
    }

    /// -1 is the channel's plain colour; otherwise a zone or a grade band.
    private func colorKey(column: Int, value: Double) -> Int {
        switch (channel, style) {
        case (.elevation, .detail):
            guard column < grades.count else { return -1 }
            return RideTimeline.gradeBand(percent: grades[column])
        case (.heartRate, .strip), (.power, .strip):
            return timeline.zone(channel, value: value) ?? -1
        default:
            return -1
        }
    }

    private func color(_ key: Int) -> Color {
        if key >= 0 { return channel == .elevation ? OBCTheme.gradeBands[key] : OBCTheme.zones[key] }
        switch channel {
        case .elevation: return OBCTheme.amber
        case .cadence: return OBCTheme.secondary
        case .speed, .heartRate, .power: return OBCTheme.ink
        }
    }
}

/// Places the cursor under a tap or a sideways drag. A drag that starts more up or down than
/// sideways never begins, so the page still scrolls over the charts. On iOS this takes a UIKit
/// pan: a SwiftUI drag inside a scroll view holds the scroll even when it ignores the movement.
struct TimelineScrub: ViewModifier {
    let length: Double
    @Binding var cursor: Double?

    func body(content: Content) -> some View {
        #if canImport(UIKit)
        content.overlay { ScrubSurface(onMove: move) }
        #else
        // macOS builds only for host tests, which never scrub.
        content
        #endif
    }

    private func move(to x: CGFloat, width: CGFloat) {
        cursor = length * Double(min(max(x / max(width, 1), 0), 1))
    }
}

#if canImport(UIKit)
private struct ScrubSurface: UIViewRepresentable {
    let onMove: (CGFloat, CGFloat) -> Void

    func makeUIView(context: Context) -> UIView {
        let view = UIView()
        view.backgroundColor = .clear
        let pan = UIPanGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.track(_:)))
        pan.delegate = context.coordinator
        view.addGestureRecognizer(pan)
        view.addGestureRecognizer(UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.track(_:))))
        return view
    }

    func updateUIView(_ view: UIView, context: Context) {
        context.coordinator.onMove = onMove
    }

    func makeCoordinator() -> Coordinator { Coordinator(onMove: onMove) }

    @MainActor final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        var onMove: (CGFloat, CGFloat) -> Void

        init(onMove: @escaping (CGFloat, CGFloat) -> Void) { self.onMove = onMove }

        @objc func track(_ recognizer: UIGestureRecognizer) {
            guard let view = recognizer.view else { return }
            onMove(recognizer.location(in: view).x, view.bounds.width)
        }

        func gestureRecognizerShouldBegin(_ recognizer: UIGestureRecognizer) -> Bool {
            guard let pan = recognizer as? UIPanGestureRecognizer else { return true }
            let velocity = pan.velocity(in: pan.view)
            return abs(velocity.x) > abs(velocity.y)
        }
    }
}
#endif

/// The columns a chart this wide draws: one per point.
func timelineColumns(width: CGFloat) -> Int {
    max(Int(width.rounded(.down)), 2)
}
