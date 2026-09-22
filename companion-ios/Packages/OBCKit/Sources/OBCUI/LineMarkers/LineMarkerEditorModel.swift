import SwiftUI
import OBCDomain

/// What the marker control reports: one begin, moves as distances, one end.
public enum LineMarkerEvent: Equatable, Sendable {
    case began(LineMarker.ID)
    case moved(LineMarker.ID, distance: Double)
    case ended(LineMarker.ID, distance: Double)
}

/// The state and the drag math behind `LineMarkerEditor`, shared by the profile and the map so
/// a move on one shows on the other in the same frame. Views only translate gestures into
/// `move` calls; every rule about where a marker may go lives here.
@MainActor
@Observable
public final class LineMarkerEditorModel {
    /// One elevation sample of the drawn profile.
    struct ProfileSample: Equatable {
        let distance: Double
        let elevation: Double
    }

    /// VoiceOver moves a marker this far per swipe.
    public static let nudgeMeters = 1000.0

    public let line: MeasuredLine
    /// Sorted by distance; `move` keeps the order.
    public private(set) var markers: [LineMarker]
    /// One colour per segment: `markers.count + 1`, in line order.
    public var segmentColors: [Color]
    /// The marker under a finger, or under VoiceOver's adjustment.
    public private(set) var activeID: LineMarker.ID?
    @ObservationIgnored public var onEvent: (LineMarkerEvent) -> Void

    /// The profile resampled by distance, so a 50,000-point line draws as a few hundred.
    let profile: [ProfileSample]
    let elevationRange: ClosedRange<Double>

    public init(
        line: MeasuredLine,
        markers: [LineMarker],
        segmentColors: [Color],
        onEvent: @escaping (LineMarkerEvent) -> Void = { _ in }
    ) {
        precondition(segmentColors.count == markers.count + 1, "one colour per segment")
        self.line = line
        self.markers = markers.sorted { $0.distance < $1.distance }
        self.segmentColors = segmentColors
        self.onEvent = onEvent

        let count = min(max(line.vertices.count, 2), 320)
        let profile = (0..<count).map { i -> ProfileSample in
            let distance = line.length * Double(i) / Double(count - 1)
            return ProfileSample(distance: distance, elevation: line.elevation(at: distance))
        }
        self.profile = profile
        let lo = profile.map(\.elevation).min() ?? 0
        let hi = profile.map(\.elevation).max() ?? 0
        elevationRange = lo...max(hi, lo + 1)
    }

    public func marker(_ id: LineMarker.ID) -> LineMarker? {
        markers.first { $0.id == id }
    }

    /// The colour of the segment that ends at this marker: the day it closes.
    func color(endingAt id: LineMarker.ID) -> Color {
        segmentColors[index(of: id) ?? 0]
    }

    /// "km 156 · 1,480 m": where the marker is and how high.
    func label(for id: LineMarker.ID, locale: Locale = .current) -> String {
        guard let marker = marker(id) else { return "" }
        let km = OBCFormat.distanceValue(meters: marker.distance, locale: locale)
        let elevation = OBCFormat.climbValue(meters: line.elevation(at: marker.distance), locale: locale)
        return "km \(km) · \(elevation) m"
    }

    // MARK: Moves

    public func begin(_ id: LineMarker.ID) {
        guard marker(id) != nil else { return }
        activeID = id
        onEvent(.began(id))
    }

    /// Move to a distance along the line, held between the marker's neighbours.
    public func move(_ id: LineMarker.ID, to distance: Double) {
        guard let index = index(of: id) else { return }
        let clamped = LineMarker.clamp(distance, forMarkerAt: index, in: markers, length: line.length)
        guard clamped != markers[index].distance else { return }
        markers[index].distance = clamped
        onEvent(.moved(id, distance: clamped))
    }

    /// Move toward a map position: the finger projects onto the line within `window`
    /// metres of the marker, so the marker follows without a jump.
    public func move(_ id: LineMarker.ID, toward coordinate: Coordinate, window: Double) {
        guard let current = marker(id)?.distance else { return }
        move(id, to: line.project(coordinate, near: current, window: window))
    }

    public func end() {
        guard let id = activeID, let marker = marker(id) else { return }
        activeID = nil
        onEvent(.ended(id, distance: marker.distance))
    }

    /// One VoiceOver step: a whole move in one call.
    public func nudge(_ id: LineMarker.ID, by meters: Double) {
        guard let current = marker(id)?.distance else { return }
        begin(id)
        move(id, to: current + meters)
        end()
    }

    private func index(of id: LineMarker.ID) -> Int? {
        markers.firstIndex { $0.id == id }
    }
}
