#if DEBUG
import SwiftUI
import OBCDomain

/// The choice for a stop off the line on the sample Alps line, Day 1 ending at a campsite beside
/// the Rhône: both modes routed, and no road found.
struct OffLineStopGallerySection: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Routed")
            sheet(GalleryLegRouter(failure: nil)).frame(height: 270)
            OBCEyebrow("No road")
            sheet(GalleryLegRouter(failure: .noRoad)).frame(height: 220)
        }
    }

    private func sheet(_ router: GalleryLegRouter) -> some View {
        var trip = GalleryStops.trip(waypoints: false)
        let camp = Stop(name: "Camping Ulrichen", coordinate: Coordinate(latitude: 46.5040, longitude: 8.3000), kind: .campsite)
        trip.endDay(0, at: trip.place([camp])[0])
        return OffLineStopSheet(
            model: OffLineStopModel(trip: trip, day: 0, router: router, onPick: { _ in }),
            color: OBCTheme.stageColor(index: 0)
        )
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusSheet).strokeBorder(OBCTheme.line))
    }
}

/// A straight road between the two points, or a failure.
struct GalleryLegRouter: LegRouter {
    let failure: LegRouteFailure?

    func route(
        from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
    ) async throws -> [RoutePoint] {
        if let failure { throw failure }
        return [RoutePoint(coordinate: from), RoutePoint(coordinate: to)]
    }
}
#endif
