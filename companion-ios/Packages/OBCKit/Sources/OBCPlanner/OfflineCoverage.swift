import Foundation

public struct OfflineCoverage: Decodable, Sendable {
    public let format: Int
    public let bounds: [Double]
    public let zoom: Int

    public func contains(_ area: [Double]) -> Bool {
        OfflineMap.valid(area) && area[0] >= bounds[0] && area[1] >= bounds[1]
            && area[2] <= bounds[2] && area[3] <= bounds[3]
    }

    public func cells(covering area: [Double]) -> [[Double]] {
        guard OfflineMap.valid(area), OfflineMap.valid(bounds), zoom == 9 else { return [] }
        let clipped = [max(area[0], bounds[0]), max(area[1], bounds[1]), min(area[2], bounds[2]), min(area[3], bounds[3])]
        guard OfflineMap.valid(clipped) else { return [] }
        let count = Double(1 << zoom)
        func row(_ lat: Double) -> Double { (1 - asinh(tan(lat * .pi / 180)) / .pi) / 2 * count }
        func latitude(_ y: Int) -> Double { atan(sinh(.pi * (1 - 2 * Double(y) / count))) * 180 / .pi }
        let left = Int(floor((clipped[0] + 180) / 360 * count)), right = Int(ceil((clipped[2] + 180) / 360 * count))
        let top = Int(floor(row(clipped[3]))), bottom = Int(ceil(row(clipped[1])))
        guard (right - left) * (bottom - top) <= 1024 else { return [] }
        var cells: [[Double]] = []
        for x in left..<right {
            let west = max(bounds[0], Double(x) / count * 360 - 180)
            let east = min(bounds[2], Double(x + 1) / count * 360 - 180)
            for y in top..<bottom { cells.append([west, max(bounds[1], latitude(y + 1)), east, min(bounds[3], latitude(y))]) }
        }
        return cells
    }
}
