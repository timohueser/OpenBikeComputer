#if DEBUG
import SwiftUI
import OBCDomain

/// The choice for a stop off the line, Day 1 ending at a campsite: on the sample Alps line with
/// both modes routed, and with no road found; and beside the start of a piece of the line, where a
/// via has no room to leave it.
struct OffLineStopGallerySection: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            OBCEyebrow("Routed")
            sheet(Self.alps(), GalleryLegRouter(failure: nil)).frame(height: 270)
            OBCEyebrow("No room for a via")
            sheet(Self.atAPieceStart(), GalleryLegRouter(failure: nil)).frame(height: 270)
            OBCEyebrow("No road")
            sheet(Self.alps(), GalleryLegRouter(failure: .noRoad)).frame(height: 220)
        }
    }

    private static func alps() -> Trip {
        var trip = GalleryStops.trip(waypoints: false)
        let camp = Stop(name: "Camping Ulrichen", coordinate: Coordinate(latitude: 46.5040, longitude: 8.3000), kind: .campsite)
        trip.endDay(0, at: trip.place([camp])[0])
        return trip
    }

    /// Three files east along one parallel, the first two 150 m apart and joined into one day. The
    /// camp lies 400 m north of where the second file starts.
    private static func atAPieceStart() -> Trip {
        func point(_ x: Double, _ y: Double = 0) -> Coordinate {
            Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
        }
        func file(_ from: Double, _ to: Double) -> [RoutePoint] {
            stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: point($0)) }
        }
        var trip = Trip.joining(
            [file(0, 10_000), file(10_150, 20_050), file(20_050, 30_050)], id: TripID("gallery-gap"),
            name: "Gap", bikeType: .gravel, now: Date(timeIntervalSince1970: 0))
        trip.removeDayEnd(0)
        let camp = Stop(name: "Camping Ulrichen", coordinate: point(10_150, 400), kind: .campsite)
        trip.endDay(0, at: trip.place([camp], near: 10_000)[0])
        return trip
    }

    private func sheet(_ trip: Trip, _ router: GalleryLegRouter) -> some View {
        OffLineStopSheet(
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
