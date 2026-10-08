import Foundation
import OBCDomain

extension RouteSummary {
    /// Saved routes and import previews use the totals that an upload writes to the device.
    public init(route: ImportedRoute, id: RouteID? = nil, bikeType: BikeType = .road, sourceFileName: String) {
        let distance: Double
        let climb: Double
        if let totals = RouteObjectCodec.totals(points: route.points) {
            distance = Double(totals.distanceMeters)
            climb = Double(totals.ascentMeters)
        } else {
            let stats = RouteStats.compute(from: route.points)
            distance = stats.distanceMeters
            climb = stats.elevationGainMeters
        }
        self.init(
            id: id ?? RouteID("imported-\(UUID().uuidString.lowercased())"),
            name: route.name ?? sourceFileName,
            distanceMeters: distance,
            elevationGainMeters: climb,
            estimatedDuration: bikeType.estimatedDuration(distanceMeters: distance, ascentMeters: climb),
            pointCount: route.points.count,
            source: (sourceFileName as NSString).pathExtension.lowercased() == "tcx" ? .tcx : .gpx,
            trackPreview: TrackPreview.normalizing(route.points.map(\.coordinate))
        )
    }
}

extension PlannedRouteRecord {
    public init(
        route: ImportedRoute,
        id: RouteID? = nil,
        bikeType: BikeType = .road,
        sourceFileName: String,
        sourceFileData: Data,
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil,
        addedAt: Date = Date(),
        plan: PlannerPlan? = nil
    ) {
        self.init(
            summary: RouteSummary(route: route, id: id, bikeType: bikeType, sourceFileName: sourceFileName),
            route: route, bikeType: bikeType, sourceFileName: sourceFileName, sourceFileData: sourceFileData,
            deviceLink: deviceLink, uploadedCRC32: uploadedCRC32, addedAt: addedAt, plan: plan
        )
    }
}
