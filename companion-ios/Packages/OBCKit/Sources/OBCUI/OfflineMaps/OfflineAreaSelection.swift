import OBCDomain
import OBCPlanner

/// Geographic bounds do not depend on the map camera or the size of its view.
struct OfflineAreaSelection: Equatable {
    enum Corner: CaseIterable { case northwest, southeast }
    private(set) var bounds: [Double]

    init?(_ bounds: [Double]) {
        guard OfflineMap.valid(bounds) else { return nil }
        self.bounds = bounds
    }

    mutating func move(_ corner: Corner, to point: Coordinate) {
        guard point.longitude.isFinite, point.latitude.isFinite else { return }
        let minimum = 0.000001
        switch corner {
        case .northwest:
            bounds[0] = max(-180, min(bounds[2] - minimum, point.longitude))
            bounds[3] = min(85, max(bounds[1] + minimum, point.latitude))
        case .southeast:
            bounds[2] = min(180, max(bounds[0] + minimum, point.longitude))
            bounds[1] = max(-85, min(bounds[3] - minimum, point.latitude))
        }
    }
}
