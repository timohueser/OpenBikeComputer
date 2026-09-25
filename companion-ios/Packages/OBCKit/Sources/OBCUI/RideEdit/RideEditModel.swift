import SwiftUI
import OBCDomain

/// The edit screen's state: the ride on the shared marker control, in trim or split mode, and
/// what Save would do. A handle's distance maps to a point of the ride, and so to a time.
@MainActor
@Observable
public final class RideEditModel {
    public enum Mode: Equatable, Sendable {
        case trim
        case split
    }

    public let editor: LineMarkerEditorModel
    public private(set) var mode: Mode = .trim
    private let points: [RidePoint]
    private let locale: Locale

    /// Nil for a ride with fewer than two points: there is nothing to edit.
    public init?(ride: Ride, locale: Locale = .current) {
        let line = MeasuredLine(ridePoints: ride.points)
        guard let editor = LineMarkerEditorModel(
            line: line, markers: Self.trimMarkers(line), segmentColors: Self.trimColors, dashedSegments: [0, 2],
            cased: false
        ) else { return nil }
        self.editor = editor
        points = ride.points
        self.locale = locale
    }

    /// A mode starts from its own handles: the trim handles at the ends, the split handle halfway.
    public func select(_ mode: Mode) {
        guard mode != self.mode else { return }
        self.mode = mode
        switch mode {
        case .trim:
            editor.setMarkers(Self.trimMarkers(line), segmentColors: Self.trimColors, dashedSegments: [0, 2])
        case .split:
            editor.setMarkers(
                [LineMarker(id: 0, distance: line.length / 2, name: "Split")],
                segmentColors: Self.splitColors
            )
        }
    }

    /// The kept time range, or nil when the handles keep the whole ride or fewer than two points.
    public var trimRange: ClosedRange<Date>? {
        guard mode == .trim, let kept = keptIndices, kept != 0...(points.count - 1) else { return nil }
        return points[kept.lowerBound].timestamp...points[kept.upperBound].timestamp
    }

    /// The time of the point both parts share, or nil when a part would have fewer than two points.
    public var splitTime: Date? {
        guard mode == .split, let index = splitIndex else { return nil }
        return points[index].timestamp
    }

    public var canSave: Bool { trimRange != nil || splitTime != nil }

    /// "Keeps 09:34 – 10:43 · 13.1 of 15.8 km" or "Splits at 10:12 · 7.4 + 8.4 km".
    public var summaryLine: String {
        let vertices = line.vertices
        switch mode {
        case .trim:
            guard let kept = keptIndices else { return "Keeps nothing" }
            let from = points[kept.lowerBound].timestamp, to = points[kept.upperBound].timestamp
            let km = vertices[kept.upperBound].distance - vertices[kept.lowerBound].distance
            return "Keeps \(OBCFormat.clock(from, locale: locale)) – \(OBCFormat.clock(to, locale: locale)) · "
                + "\(OBCFormat.distanceValue(meters: km, locale: locale)) of "
                + OBCFormat.distance(meters: line.length, locale: locale)
        case .split:
            let index = splitIndex ?? line.index(at: editor.markers.first?.distance ?? 0)
            let before = vertices[index].distance
            return "Splits at \(OBCFormat.clock(points[index].timestamp, locale: locale)) · "
                + "\(OBCFormat.distanceValue(meters: before, locale: locale)) + "
                + OBCFormat.distance(meters: line.length - before, locale: locale)
        }
    }

    private var line: MeasuredLine { editor.line }

    /// The first point at or after the start handle through the last point at or before the end
    /// handle, so no cut part stays.
    private var keptIndices: ClosedRange<Int>? {
        guard editor.markers.count == 2 else { return nil }
        let start = editor.markers[0].distance, end = editor.markers[1].distance
        let before = line.index(at: start)
        let first = line.vertices[before].distance < start ? before + 1 : before
        let last = line.index(at: end)
        return first < last ? first...last : nil
    }

    private var splitIndex: Int? {
        guard let marker = editor.markers.first else { return nil }
        let before = line.index(at: marker.distance)
        let vertices = line.vertices
        // The nearest point, so the handle and the split agree.
        let index = before + 1 < vertices.count
            && vertices[before + 1].distance - marker.distance < marker.distance - vertices[before].distance
            ? before + 1 : before
        return index > 0 && index < points.count - 1 ? index : nil
    }

    private static func trimMarkers(_ line: MeasuredLine) -> [LineMarker] {
        [LineMarker(id: 0, distance: 0, name: "Trim start"), LineMarker(id: 1, distance: line.length, name: "Trim end")]
    }

    /// The kept ride in the ride colour; the cut ends faint and dashed.
    private static let trimColors = [OBCTheme.secondary.opacity(0.55), OBCTheme.ride, OBCTheme.secondary.opacity(0.55)]
    /// Two rides after the split: the ride colour, then the next blue, never a planned magenta.
    private static let splitColors = [OBCTheme.ride, OBCTheme.day2]
}
