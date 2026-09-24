import SwiftUI
import OBCDomain

/// What the device's route overview shows for one route it holds.
public struct DeviceRouteOverview: Equatable, Sendable {
    public let name: String
    public let distanceMeters: Double
    /// The estimate the device computes for the route and its bike type.
    public let estimatedDuration: TimeInterval?
    public let track: TrackPreview?

    public init(name: String, distanceMeters: Double, estimatedDuration: TimeInterval?, track: TrackPreview?) {
        self.name = name
        self.distanceMeters = distanceMeters
        self.estimatedDuration = estimatedDuration
        self.track = track
    }

    public init(_ summary: RouteSummary) {
        self.init(
            name: summary.name,
            distanceMeters: summary.distanceMeters,
            estimatedDuration: summary.estimatedDuration,
            track: summary.trackPreview
        )
    }

    /// The name cut to the title bar as the firmware cuts it: 15 cells, or 13 and "..".
    var title: String {
        name.count <= 15 ? name : String(name.prefix(13)).trimmingCharacters(in: .whitespaces) + ".."
    }

    /// Whole kilometres, rounded half up.
    var distanceValue: String { "\(Int(distanceMeters / 1000 + 0.5))" }

    /// H:MM, whole minutes floored.
    var timeValue: String {
        guard let estimatedDuration else { return "-:--" }
        let minutes = Int(estimatedDuration / 60)
        return String(format: "%d:%02d", minutes / 60, minutes % 60)
    }
}

/// The device's route overview page in its exact on-glass colours: the title bar, the track
/// shape, the DISTANCE and EST TIME ledger and the action rows. Drawn in the 240 x 320 panel's
/// own pixel coordinates, taken from the firmware's layout, and scaled to the frame.
struct DeviceRouteOverviewScreen: View {
    let overview: DeviceRouteOverview

    var body: some View {
        Canvas { context, size in
            context.scaleBy(x: size.width / 240, y: size.height / 320)
            context.fill(Path(CGRect(x: 0, y: 0, width: 240, height: 320)), with: .color(.white))
            context.stroke(
                Path(roundedRect: CGRect(x: 4.5, y: 4.5, width: 231, height: 311), cornerRadius: 8),
                with: .color(OBCTheme.deviceRule), lineWidth: 1
            )
            context.fill(
                Path(roundedRect: CGRect(x: 4, y: 4, width: 232, height: 34), cornerRadius: 6),
                with: .color(OBCTheme.deviceHeader)
            )
            text(overview.title, .body, x: 14, capTop: 12, color: OBCTheme.deviceHeaderText, in: &context)

            drawTrack(in: &context)

            context.fill(Path(CGRect(x: 12, y: 141, width: 216, height: 1)), with: .color(OBCTheme.deviceRule))
            ledgerRow("DISTANCE", value: overview.distanceValue, unit: "km", capTop: 164, in: &context)
            context.fill(Path(CGRect(x: 16, y: 184, width: 208, height: 1)), with: .color(OBCTheme.deviceRule))
            ledgerRow("EST TIME", value: overview.timeValue, unit: "h", capTop: 206, in: &context)

            context.fill(
                Path(roundedRect: CGRect(x: 14, y: 226, width: 212, height: 38), cornerRadius: 5),
                with: .color(OBCTheme.deviceTrack)
            )
            text("START RIDE", .body, x: 26, capTop: 235, color: OBCTheme.deviceInk, in: &context)
            text("Delete route", .body, x: 26, capTop: 281, color: OBCTheme.deviceInk, in: &context)
        }
    }

    /// Caption left, unit right, value right-aligned before the unit, one baseline.
    private func ledgerRow(_ caption: String, value: String, unit: String, capTop: CGFloat, in context: inout GraphicsContext) {
        text(caption, .label, x: 16, capTop: capTop, color: OBCTheme.deviceCaption, in: &context)
        text(unit, .label, x: 224, capTop: capTop, color: OBCTheme.deviceCaption, alignRight: true, in: &context)
        let valueRight = 224 - CGFloat(unit.count * PixelFont.label.cellWidth) - 6
        text(value, .display, x: valueRight, capTop: capTop - 6, color: OBCTheme.deviceInk, alignRight: true, in: &context)
    }

    /// The track in a 212 x 90 box between the title bar and the first rule: a two-pixel ink
    /// line from a filled start disc to a hollow end diamond.
    private func drawTrack(in context: inout GraphicsContext) {
        guard let track = overview.track, track.points.count > 1 else { return }
        let box = CGRect(x: 14, y: 44, width: 212, height: 90)
        let fit = TrackPreviewView.fittingTransform(for: track, in: box.size, inset: 4)
        let points = track.points.map { fit($0) }.map { CGPoint(x: $0.x + box.minX, y: $0.y + box.minY) }
        var line = Path()
        line.addLines(points)
        context.stroke(line, with: .color(OBCTheme.deviceInk), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))

        let start = points[0]
        context.fill(Path(ellipseIn: CGRect(x: start.x - 2.5, y: start.y - 2.5, width: 5, height: 5)), with: .color(OBCTheme.deviceInk))
        let end = points[points.count - 1]
        var diamond = Path()
        diamond.addLines([
            CGPoint(x: end.x, y: end.y - 3.5), CGPoint(x: end.x + 3.5, y: end.y),
            CGPoint(x: end.x, y: end.y + 3.5), CGPoint(x: end.x - 3.5, y: end.y),
        ])
        diamond.closeSubpath()
        context.fill(diamond, with: .color(.white))
        context.stroke(diamond, with: .color(OBCTheme.deviceInk), lineWidth: 1)
    }

    /// Terminus text placed by its cap top, so strings with and without ascenders share a baseline.
    private func text(
        _ string: String, _ font: PixelFont, x: CGFloat, capTop: CGFloat, color: Color,
        alignRight: Bool = false, in context: inout GraphicsContext
    ) {
        let bitmap = PixelBitmap(string, font: font)
        let cellTop = capTop - CGFloat(PixelBitmap("H", font: font).inkTop)
        let origin = CGPoint(
            x: alignRight ? x - CGFloat(bitmap.width) : x,
            y: cellTop + CGFloat(bitmap.inkTop)
        )
        var path = Path()
        for run in bitmap.runs {
            path.addRect(CGRect(x: origin.x + CGFloat(run.x), y: origin.y + CGFloat(run.y), width: CGFloat(run.length), height: 1))
        }
        context.fill(path, with: .color(color))
    }
}
