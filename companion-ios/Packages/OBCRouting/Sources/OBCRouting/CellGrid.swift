import Foundation
import OBCDomain

/// One square of the OBCA grid (`specs/OBCA_Spec.md` §1): a size as `log2(µdeg)`, a latitude
/// index `i` and a longitude index `j`.
struct CellID: Hashable, Comparable, Sendable {
    static let origin = -268_435_456
    static let worldSide = 536_870_912

    let log2: Int
    let i: Int
    let j: Int

    /// Parses `<log2>/<i>/<j>`, lenient about zero padding.
    init?(_ text: String) {
        let parts = text.split(separator: "/", omittingEmptySubsequences: false).map { Int($0) }
        guard parts.count == 3, let log2 = parts[0], let i = parts[1], let j = parts[2],
              (10...28).contains(log2), i >= 0, j >= 0,
              i < Self.worldSide >> log2, j < Self.worldSide >> log2 else { return nil }
        self.init(log2: log2, i: i, j: j)
    }

    init(log2: Int, i: Int, j: Int) {
        self.log2 = log2
        self.i = i
        self.j = j
    }

    /// The canonical id: indices padded to `max(4, digits(cells per axis − 1))`.
    var id: String {
        let width = max(4, String((Self.worldSide >> log2) - 1).count)
        func pad(_ v: Int) -> String {
            let digits = String(v)
            return String(repeating: "0", count: max(0, width - digits.count)) + digits
        }
        return "\(log2)/\(pad(i))/\(pad(j))"
    }

    static func < (a: CellID, b: CellID) -> Bool { (a.log2, a.i, a.j) < (b.log2, b.i, b.j) }

    /// Every cell of size `2^log2` that the box spanning `a` and `b`, grown by `marginMeters`
    /// on every side, touches.
    static func covering(_ a: Coordinate, _ b: Coordinate, marginMeters: Double, log2: Int) -> [CellID] {
        let metersPerMicrodegree = 0.11132
        let poleward = max(abs(a.latitude), abs(b.latitude))
        let dLat = (marginMeters / metersPerMicrodegree).rounded(.up)
        let dLon = (dLat / max(cos(poleward * .pi / 180), 0.01)).rounded(.up)
        let last = (worldSide >> log2) - 1
        func index(_ degrees: Double, _ pad: Double) -> Int {
            let v = degrees * 1_000_000 + pad
            return min(max(Int(((v - Double(origin)) / Double(1 << log2)).rounded(.down)), 0), last)
        }
        var cells: [CellID] = []
        for i in index(min(a.latitude, b.latitude), -dLat)...index(max(a.latitude, b.latitude), dLat) {
            for j in index(min(a.longitude, b.longitude), -dLon)...index(max(a.longitude, b.longitude), dLon) {
                cells.append(CellID(log2: log2, i: i, j: j))
            }
        }
        return cells
    }
}
