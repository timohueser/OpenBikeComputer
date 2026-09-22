import Foundation

/// A marker on a `MeasuredLine`: a day end, or a trim bound. Markers never cross, so a set
/// of them stays sorted by distance and each one moves only between its neighbours.
public struct LineMarker: Identifiable, Equatable, Sendable {
    public let id: Int
    /// Metres along the line.
    public var distance: Double
    /// What VoiceOver calls it: "Day 2 end".
    public var name: String

    public init(id: Int, distance: Double, name: String) {
        self.id = id
        self.distance = distance
        self.name = name
    }

    /// `distance` held inside the line and between the neighbours of `markers[index]`.
    public static func clamp(
        _ distance: Double, forMarkerAt index: Int, in markers: [LineMarker], length: Double
    ) -> Double {
        let low = index > 0 ? markers[index - 1].distance : 0
        let high = index + 1 < markers.count ? markers[index + 1].distance : length
        return min(max(distance, low), high)
    }
}
