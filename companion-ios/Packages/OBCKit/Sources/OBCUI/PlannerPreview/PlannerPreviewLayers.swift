#if DEBUG && os(iOS)
import MapKit

// Keep these tokens aligned with builder/app/src/lib/planner/route-overlays.ts.
enum PlannerPreviewNetworkStyle {
    static func color(rank: Int, traits: UITraitCollection) -> UIColor {
        let colors: [UInt32] = traits.userInterfaceStyle == .dark
            ? [0xb0b8be, 0xa4cf67, 0x79b7f1, 0xc49de0]
            : [0x626a70, 0x4f8b24, 0x2368b5, 0x7c519c]
        let value = colors[min(3, max(0, rank))]
        return UIColor(red: CGFloat((value >> 16) & 255) / 255,
                       green: CGFloat((value >> 8) & 255) / 255,
                       blue: CGFloat(value & 255) / 255, alpha: 0.8)
    }

    static func zoom(_ scale: MKZoomScale) -> Double { log2(Double(scale) * MKMapSize.world.width / 512) }

    static func width(zoom: Double) -> CGFloat {
        let stops: [(Double, Double)] = [(6, 1.2), (10, 1.8), (13, 3.2), (17, 5)]
        for pair in zip(stops, stops.dropFirst()) where zoom < pair.1.0 {
            let fraction = max(0, (zoom - pair.0.0) / (pair.1.0 - pair.0.0))
            return pair.0.1 + fraction * (pair.1.1 - pair.0.1)
        }
        return 5
    }
}

final class PlannerPreviewNetworkOverlay: MKMultiPolyline {
    var rank = 0
    var network = ""
    var minimumZoom: Double { rank >= 3 ? 6 : rank == 2 ? 8 : 10 }
}

final class PlannerPreviewNetworkRenderer: MKMultiPolylineRenderer {
    override func draw(_ mapRect: MKMapRect, zoomScale: MKZoomScale, in context: CGContext) {
        guard let network = overlay as? PlannerPreviewNetworkOverlay else { return }
        let zoom = PlannerPreviewNetworkStyle.zoom(zoomScale)
        guard zoom >= network.minimumZoom else { return }
        lineWidth = PlannerPreviewNetworkStyle.width(zoom: zoom)
        super.draw(mapRect, zoomScale: zoomScale, in: context)
    }
}

final class PlannerPreviewNetworkLabel: MKPointAnnotation {
    let network: String
    let rank: Int
    init(network: String, rank: Int, text: String, at point: CLLocationCoordinate2D) {
        self.network = network
        self.rank = rank
        super.init()
        title = text
        coordinate = point
    }

    func image(traits: UITraitCollection) -> UIImage {
        let text = title ?? ""
        let attributes: [NSAttributedString.Key: Any] = [
            .font: UIFont.systemFont(ofSize: 11, weight: .medium),
            .foregroundColor: PlannerPreviewNetworkStyle.color(rank: rank, traits: traits).withAlphaComponent(1),
            .strokeColor: traits.userInterfaceStyle == .dark ? UIColor(red: 24 / 255, green: 29 / 255, blue: 25 / 255, alpha: 1) : .white,
            .strokeWidth: -4,
        ]
        let size = (text as NSString).size(withAttributes: attributes)
        return UIGraphicsImageRenderer(size: CGSize(width: size.width + 6, height: size.height + 6)).image { _ in
            (text as NSString).draw(at: CGPoint(x: 3, y: 3), withAttributes: attributes)
        }
    }
}

@MainActor
enum PlannerPreviewNetworkData {
    private struct Collection: Decodable { let features: [Feature] }
    private struct Feature: Decodable {
        struct Properties: Decodable { let kind: String; let rank: Int; let ref: String }
        struct Geometry: Decodable { let coordinates: [[Double]] }
        let properties: Properties
        let geometry: Geometry
    }

    /// The app target carries the fixture in Debug only, so it never ships in Release.
    private static let features: [Feature] = {
        guard let url = Bundle.main.url(forResource: "PlannerPreviewNetworks", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let collection = try? JSONDecoder().decode(Collection.self, from: data) else {
            assertionFailure("The bundled planner network fixture must decode.")
            return []
        }
        return collection.features
    }()

    static func overlays(network: String) -> [PlannerPreviewNetworkOverlay] {
        let groups = Dictionary(grouping: features.filter { $0.properties.kind == network }, by: { $0.properties.rank })
        return groups.keys.sorted().map { rank in
            let lines = groups[rank, default: []].map { feature in
                let points = feature.geometry.coordinates.map { CLLocationCoordinate2D(latitude: $0[1], longitude: $0[0]) }
                return MKPolyline(coordinates: points, count: points.count)
            }
            let overlay = PlannerPreviewNetworkOverlay(lines)
            overlay.rank = rank
            overlay.network = network
            return overlay
        }
    }

    static func labels(network: String) -> [PlannerPreviewNetworkLabel] {
        var labels: [PlannerPreviewNetworkLabel] = []
        var positions: [String: [MKMapPoint]] = [:]
        for feature in features where feature.properties.kind == network && !feature.properties.ref.isEmpty {
            let coordinates = feature.geometry.coordinates
            let middle = coordinates[coordinates.count / 2]
            let coordinate = CLLocationCoordinate2D(latitude: middle[1], longitude: middle[0])
            let point = MKMapPoint(coordinate)
            let text = feature.properties.ref
            guard !positions[text, default: []].contains(where: { $0.distance(to: point) < 1_200 }) else { continue }
            positions[text, default: []].append(point)
            labels.append(PlannerPreviewNetworkLabel(network: network, rank: feature.properties.rank, text: text, at: coordinate))
        }
        return labels
    }
}
#endif
