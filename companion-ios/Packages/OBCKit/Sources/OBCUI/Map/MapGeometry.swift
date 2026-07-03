#if canImport(MapKit)
import MapKit
import OBCDomain

/// Small MapKit adapters shared by the basemap preview and the interactive map
/// (#294). Kept out of the views so the projection/camera math has one home.
enum MapGeometry {
    static func clLocation(_ coordinate: Coordinate) -> CLLocationCoordinate2D {
        CLLocationCoordinate2D(latitude: coordinate.latitude, longitude: coordinate.longitude)
    }

    static func clLocations(_ coordinates: [Coordinate]) -> [CLLocationCoordinate2D] {
        coordinates.map(clLocation)
    }

    /// A region that frames the whole track with a little breathing room. `pad`
    /// is a multiplier on the track's span (1.3 = 30% margin). A single-point or
    /// zero-span track gets a small fixed span so the camera isn't fully zoomed.
    static func boundingRegion(for coordinates: [Coordinate], pad: Double = 1.3) -> MKCoordinateRegion {
        guard let first = coordinates.first else {
            return MKCoordinateRegion(
                center: CLLocationCoordinate2D(latitude: 0, longitude: 0),
                span: MKCoordinateSpan(latitudeDelta: 1, longitudeDelta: 1)
            )
        }
        var minLat = first.latitude, maxLat = first.latitude
        var minLon = first.longitude, maxLon = first.longitude
        for c in coordinates {
            minLat = min(minLat, c.latitude); maxLat = max(maxLat, c.latitude)
            minLon = min(minLon, c.longitude); maxLon = max(maxLon, c.longitude)
        }
        let center = CLLocationCoordinate2D(
            latitude: (minLat + maxLat) / 2,
            longitude: (minLon + maxLon) / 2
        )
        // A minimum span keeps a tiny/degenerate track from zooming to the max.
        let span = MKCoordinateSpan(
            latitudeDelta: max((maxLat - minLat) * pad, 0.004),
            longitudeDelta: max((maxLon - minLon) * pad, 0.004)
        )
        return MKCoordinateRegion(center: center, span: span)
    }
}
#endif
