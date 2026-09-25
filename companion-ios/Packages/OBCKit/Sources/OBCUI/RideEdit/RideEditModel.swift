import SwiftUI
import OBCDomain

/// The edit screen's state: the ride on the shared marker control with its two trim handles, and
/// what Save would keep. A handle's distance maps to a point of the ride, and so to a time.
@MainActor
@Observable
public final class RideEditModel {
    public let editor: LineMarkerEditorModel
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

    /// The kept time range, or nil when the handles keep the whole ride or fewer than two points.
    public var trimRange: ClosedRange<Date>? {
        guard let kept = keptIndices, kept != 0...(points.count - 1) else { return nil }
        return points[kept.lowerBound].timestamp...points[kept.upperBound].timestamp
    }

    public var canSave: Bool { trimRange != nil }

    /// "Keeps 09:34 – 10:43 · 13.1 of 15.8 km".
    public var summaryLine: String {
        guard let kept = keptIndices else { return "Keeps nothing" }
        let vertices = line.vertices
        let from = points[kept.lowerBound].timestamp, to = points[kept.upperBound].timestamp
        let km = vertices[kept.upperBound].distance - vertices[kept.lowerBound].distance
        return "Keeps \(OBCFormat.clock(from, locale: locale)) – \(OBCFormat.clock(to, locale: locale)) · "
            + "\(OBCFormat.distanceValue(meters: km, locale: locale)) of "
            + OBCFormat.distance(meters: line.length, locale: locale)
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

    private static func trimMarkers(_ line: MeasuredLine) -> [LineMarker] {
        [LineMarker(id: 0, distance: 0, name: "Trim start"), LineMarker(id: 1, distance: line.length, name: "Trim end")]
    }

    /// The kept ride in the ride colour; the cut ends faint and dashed.
    private static let trimColors = [OBCTheme.secondary.opacity(0.55), OBCTheme.ride, OBCTheme.secondary.opacity(0.55)]
}
