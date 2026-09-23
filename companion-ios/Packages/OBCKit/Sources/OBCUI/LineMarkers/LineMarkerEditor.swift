import SwiftUI
import OBCDomain

/// Markers on a line, on the map and on the elevation profile at once: the day-end handles of
/// the day editor and the trim handles of ride editing. Drag a handle on either view and the
/// other follows in the same frame. Offline, the map is the grid preview and only the profile
/// drags.
public struct LineMarkerEditor: View {
    let model: LineMarkerEditorModel
    var mapHeight: CGFloat

    @Environment(\.obcIsOnline) private var isOnline

    public init(model: LineMarkerEditorModel, mapHeight: CGFloat = 240) {
        self.model = model
        self.mapHeight = mapHeight
    }

    public var body: some View {
        VStack(spacing: 12) {
            map
                .frame(height: mapHeight)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
                .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
            LineMarkerProfileView(model: model)
        }
        // One tick on grab and one on release, from either view.
        .sensoryFeedback(.impact(weight: .light), trigger: model.activeID)
    }

    @ViewBuilder
    private var map: some View {
        let mode = MapPreviewMode.resolve(isOnline: isOnline, hasCoordinates: model.line.vertices.count > 1)
        #if canImport(UIKit) && canImport(MapKit)
        if mode == .map {
            LineMarkerMapView(
                model: model,
                lineVersion: model.lineVersion,
                markers: model.markers,
                activeID: model.activeID,
                segmentColors: model.segmentColors,
                stops: model.stops
            )
        } else {
            grid
        }
        #else
        grid
        #endif
    }

    /// The offline map: the segments as stages of the shared grid preview.
    private var grid: some View {
        let bounds = [0] + model.markers.map(\.distance) + [model.line.length]
        let stages = (0..<(bounds.count - 1)).map { segment in
            let from = bounds[segment], to = bounds[segment + 1]
            let first = model.line.index(at: from), last = model.line.index(at: to)
            let inner = first + 1 <= last ? model.line.vertices[(first + 1)...last].map(\.coordinate) : []
            return MultiTrackPreviewView.Stage(
                coordinates: [model.line.coordinate(at: from)] + inner + [model.line.coordinate(at: to)],
                color: model.segmentColors[segment]
            )
        }
        return MultiTrackPreviewView(stages: stages, showsChrome: false)
    }
}
