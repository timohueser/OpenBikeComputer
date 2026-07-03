import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// The full-screen **interactive** track map (#294) — default MapKit pinch /
/// zoom / pan over the same halo + stroke polyline and start/end dots the
/// previews draw, framed to the track's bounds on open. Presented as a
/// `fullScreenCover` from the route/ride detail hero.
///
/// Only reached when there's real geometry and a network path (the detail hero
/// only offers the tap when online), so there's no offline/blank-map state to
/// draw here — the grid preview stays put when offline.
public struct TrackMapView: View {
    private let coordinates: [Coordinate]
    private let title: String
    private let onClose: () -> Void

    public init(coordinates: [Coordinate], title: String, onClose: @escaping () -> Void) {
        self.coordinates = coordinates
        self.title = title
        self.onClose = onClose
    }

    public var body: some View {
        NavigationStack {
            mapBody
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle(title)
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done", action: onClose)
                            .fontWeight(.semibold)
                    }
                }
                .accessibilityIdentifier("trackMap.screen")
        }
        .tint(OBCTheme.tint)
    }

    @ViewBuilder
    private var mapBody: some View {
        #if canImport(MapKit)
        Map(initialPosition: .region(MapGeometry.boundingRegion(for: coordinates, pad: 1.4))) {
            TrackMapContent(coordinates: coordinates, dotRadius: 7)
        }
        .mapControls {
            MapCompass()
            MapScaleView()
        }
        #else
        // No MapKit (host build) → nothing interactive to show.
        Color(OBCTheme.parchment)
        #endif
    }
}

#if DEBUG
#Preview("Track map") {
    TrackMapView(
        coordinates: TrackPreview.obcSample.coordinates,
        title: "Kettle Moraine Loop",
        onClose: {}
    )
}
#endif
