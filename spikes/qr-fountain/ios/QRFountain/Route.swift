import Foundation
import SwiftUI

/// The route payload that web/route.js builds.
struct Route {
    /// Longitude, latitude in degrees.
    let points: [SIMD2<Double>]

    init?(payload: Data) {
        // Apple's .zlib is raw DEFLATE (RFC 1951), the same as the browser's 'deflate-raw'.
        guard let raw = try? (payload as NSData).decompressed(using: .zlib) else { return nil }
        var reader = VarintReader(bytes: [UInt8](raw as Data))
        guard let count = reader.next() else { return nil }
        var lat = 0, lon = 0
        var points: [SIMD2<Double>] = []
        for _ in 0 ..< count {
            guard let dLat = reader.nextSigned(), let dLon = reader.nextSigned(), reader.nextSigned() != nil else { return nil }
            lat += dLat
            lon += dLon
            points.append(SIMD2(Double(lon) / 1e6, Double(lat) / 1e6))
        }
        self.points = points
    }

    var kilometres: Double {
        zip(points, points.dropFirst()).reduce(0.0) { sum, pair in
            let (a, b) = pair
            let dx = (b.x - a.x) * cos(a.y * .pi / 180), dy = b.y - a.y
            return sum + (dx * dx + dy * dy).squareRoot() * 111.32
        }
    }
}

private struct VarintReader {
    let bytes: [UInt8]
    var offset = 0

    mutating func next() -> Int? {
        var value = 0, shift = 0
        while offset < bytes.count, shift < 35 {
            let byte = bytes[offset]
            offset += 1
            value |= Int(byte & 0x7F) << shift
            if byte & 0x80 == 0 { return value }
            shift += 7
        }
        return nil
    }

    mutating func nextSigned() -> Int? {
        next().map { ($0 >> 1) ^ -($0 & 1) }
    }
}

/// The route as a line, scaled to fit, with longitude shrunk by cos(latitude).
struct RouteShape: Shape {
    let points: [SIMD2<Double>]

    func path(in rect: CGRect) -> Path {
        guard let first = points.first else { return Path() }
        let k = cos(first.y * .pi / 180)
        let xs = points.map { $0.x * k }, ys = points.map(\.y)
        let minX = xs.min()!, maxX = xs.max()!, minY = ys.min()!, maxY = ys.max()!
        let scale = min(Double(rect.width) / max(maxX - minX, 1e-9), Double(rect.height) / max(maxY - minY, 1e-9))
        let cx = Double(rect.midX) - (minX + maxX) / 2 * scale
        let cy = Double(rect.midY) + (minY + maxY) / 2 * scale
        var path = Path()
        for (x, y) in zip(xs, ys) {
            let p = CGPoint(x: cx + x * scale, y: cy - y * scale)
            if path.isEmpty { path.move(to: p) } else { path.addLine(to: p) }
        }
        return path
    }
}
