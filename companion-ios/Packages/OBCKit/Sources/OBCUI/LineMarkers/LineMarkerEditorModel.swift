import SwiftUI
import OBCDomain

/// What the marker control reports: one begin, moves as distances, one end. A cancelled
/// drag ends like any other.
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
    /// The map's projection window is the finger's travel this frame, doubled, held between
    /// these bounds: the marker can never outrun the finger along the line, so the other leg
    /// of an out-and-back or a switchback is out of reach however far the map is zoomed out.
    public static let mapWindowBounds = 50.0...2_000.0

    public let line: MeasuredLine
    /// Sorted by distance; `move` keeps the order.
    public private(set) var markers: [LineMarker]
    /// One colour per segment: `markers.count + 1`, in line order.
    public let segmentColors: [Color]
    /// Segments drawn dashed: the cut parts of a trim.
    public let dashedSegments: Set<Int>
    /// Whether the solid segments sit on the amber casing: a planned line does, a ride does not.
    public let cased: Bool
    /// The marker under a finger, or under VoiceOver's adjustment. One at a time.
    public private(set) var activeID: LineMarker.ID?
    /// The markers as they stood before the drag in flight: what the static profile layer and
    /// the map's colour runs draw until the finger lets go.
    public private(set) var restingMarkers: [LineMarker]
    /// The part of the line the profile shows, and the range a drag may take.
    public private(set) var window: ClosedRange<Double>
    @ObservationIgnored public var onEvent: (LineMarkerEvent) -> Void
    /// A finger that touched a handle and lifted without moving it.
    @ObservationIgnored public var onTap: (LineMarker.ID) -> Void = { _ in }

    /// The window resampled by distance: a 50,000-point line draws as a few hundred samples,
    /// and a 1 km window of a 600 km line keeps its shape.
    private(set) var profile: [ProfileSample] = []
    /// About one sample per point of plot width.
    static let profileSamples = 400
    private(set) var elevationRange: ClosedRange<Double> = 0...1

    /// `nil` for a line with fewer than two vertices: there is nothing to put a marker on.
    public init?(
        line: MeasuredLine,
        markers: [LineMarker],
        segmentColors: [Color],
        dashedSegments: Set<Int> = [],
        cased: Bool = true,
        onEvent: @escaping (LineMarkerEvent) -> Void = { _ in }
    ) {
        guard line.vertices.count > 1 else { return nil }
        precondition(segmentColors.count == markers.count + 1, "one colour per segment")
        let ordered = Self.ordered(markers, on: line)
        self.line = line
        self.markers = ordered
        restingMarkers = ordered
        window = 0...max(line.length, 1)
        self.segmentColors = segmentColors
        self.dashedSegments = dashedSegments
        self.cased = cased
        self.onEvent = onEvent
        setWindow(0...line.length)
    }

    // MARK: The profile window

    /// The window held inside the line, sampled afresh; the elevation scale follows it.
    func setWindow(_ range: ClosedRange<Double>) {
        let low = min(max(range.lowerBound, 0), line.length)
        window = low...max(min(range.upperBound, line.length), low + 1)
        let count = min(max(line.vertices.count, 2), Self.profileSamples)
        let span = window.upperBound - window.lowerBound
        profile = (0..<count).map { i -> ProfileSample in
            let distance = window.lowerBound + span * Double(i) / Double(count - 1)
            return ProfileSample(distance: distance, elevation: line.elevation(at: distance))
        }
        let lo = profile.map(\.elevation).min() ?? 0
        let hi = profile.map(\.elevation).max() ?? 0
        elevationRange = lo...max(hi, lo + 1)
    }

    private static func ordered(_ markers: [LineMarker], on line: MeasuredLine) -> [LineMarker] {
        markers
            .map { marker in
                var held = marker
                held.distance = min(max(marker.distance, 0), line.length)
                return held
            }
            .sorted { $0.distance < $1.distance }
    }

    // MARK: Lookups

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

    // MARK: Grabbing

    /// Of the markers under one finger on the profile, the one that can move the way the
    /// finger goes: forward, the last of them; backward, the first. The others are blocked
    /// by it.
    func grab(among ids: [LineMarker.ID], forward: Bool) -> LineMarker.ID? {
        let candidates = markers.filter { ids.contains($0.id) }
        return forward ? candidates.last?.id : candidates.first?.id
    }

    /// Of the markers under one finger on the map, the one whose leg the finger follows:
    /// the smallest projection error wins, and a tie goes by direction as on the profile.
    /// On a loop ride the trim start and end share a map point but not a leg.
    func grab(among ids: [LineMarker.ID], toward coordinate: Coordinate, window: Double) -> LineMarker.ID? {
        let candidates = markers.filter { ids.contains($0.id) }.map { marker in
            let projection = line.projection(of: coordinate, near: marker.distance, window: window)
            return (marker: marker, error: projection.error, forward: projection.distance > marker.distance)
        }
        guard let least = candidates.map(\.error).min() else { return nil }
        let nearest = candidates.filter { $0.error <= least + MeasuredLine.tieMeters }
        let forward = nearest.contains { $0.forward }
        return forward ? nearest.last?.marker.id : nearest.first?.marker.id
    }

    /// The projection window for a map drag whose finger travelled `travelMeters` this frame.
    public static func mapWindow(travelMeters: Double) -> Double {
        min(max(travelMeters * 2, mapWindowBounds.lowerBound), mapWindowBounds.upperBound)
    }

    // MARK: Moves

    /// Take the marker. Refused for a fixed marker, and while another marker is held, so a
    /// second finger changes nothing until the first lets go.
    @discardableResult
    public func begin(_ id: LineMarker.ID) -> Bool {
        guard activeID == nil, let marker = marker(id), !marker.isFixed else { return false }
        activeID = id
        onEvent(.began(id))
        return true
    }

    /// Move the held marker to a distance along the line, between its neighbours and inside the
    /// window. Any
    /// other marker, held by nobody or by another finger, stays.
    public func move(_ id: LineMarker.ID, to distance: Double) {
        guard activeID == id, let index = index(of: id) else { return }
        let held = LineMarker.clamp(distance, forMarkerAt: index, in: markers, length: line.length)
        let clamped = min(max(held, window.lowerBound), window.upperBound)
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

    /// Let go. A second call, or a call with nothing held, does nothing, so a cancelled
    /// gesture and its late `ended` end the drag once.
    public func end() {
        guard let id = activeID, let marker = marker(id) else { return }
        activeID = nil
        restingMarkers = markers
        onEvent(.ended(id, distance: marker.distance))
    }

    /// A touch on the handle that never became a drag.
    public func tap(_ id: LineMarker.ID) {
        guard marker(id) != nil else { return }
        onTap(id)
    }

    /// One VoiceOver step: a whole move in one call.
    public func nudge(_ id: LineMarker.ID, by meters: Double) {
        guard let current = marker(id)?.distance, begin(id) else { return }
        move(id, to: current + meters)
        end()
    }

    private func index(of id: LineMarker.ID) -> Int? {
        markers.firstIndex { $0.id == id }
    }
}
