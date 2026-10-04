#if DEBUG
import SwiftUI
import OBCDomain

/// The marker-on-line editor with sample data: a ride with trim handles. The dense-line toggle is
/// the load for the frame-rate check.
struct LineMarkerGallerySection: View {
    @State private var dense = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Toggle("50,000 points", isOn: $dense)
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .tint(OBCTheme.tint)

            TrimEditorSample(line: dense ? SampleLine.alpsDense : SampleLine.alps)
                .id(dense)
        }
    }
}

private struct TrimEditorSample: View {
    @State private var model: LineMarkerEditorModel

    init(line: MeasuredLine) {
        _model = State(initialValue: LineMarkerEditorModel(
            line: line,
            markers: [
                LineMarker(id: 1, distance: 2_500, name: "Trim start"),
                LineMarker(id: 2, distance: line.length - 4_000, name: "Trim end"),
            ],
            segmentColors: [OBCTheme.secondary.opacity(0.55), OBCTheme.ride, OBCTheme.secondary.opacity(0.55)],
            dashedSegments: [0, 2],
            cased: false
        )!)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            LineMarkerEditor(model: model, mapHeight: 180)
            let kept = model.markers[1].distance - model.markers[0].distance
            Text("Keeps \(OBCFormat.distance(meters: kept)) of \(OBCFormat.distance(meters: model.line.length))")
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
        }
    }
}

/// Andermatt over the Furka to Brig, with elevations: about 80 km.
enum SampleLine {
    static let alps: MeasuredLine = {
        let points: [(Double, Double, Double)] = [
            (46.6356, 8.5946, 1437), (46.6290, 8.5820, 1445), (46.6200, 8.5680, 1452),
            (46.6120, 8.5400, 1490), (46.6040, 8.5200, 1520), (46.5980, 8.5030, 1538),
            (46.5940, 8.4850, 1620), (46.5910, 8.4720, 1780), (46.5880, 8.4660, 1950),
            (46.5900, 8.4560, 2106), (46.5840, 8.4420, 2260), (46.5780, 8.4260, 2380),
            (46.5720, 8.4150, 2429), (46.5760, 8.3960, 2300), (46.5720, 8.3800, 2100),
            (46.5680, 8.3700, 1900), (46.5620, 8.3600, 1757), (46.5500, 8.3560, 1500),
            (46.5350, 8.3500, 1368), (46.5200, 8.3300, 1355), (46.5050, 8.3090, 1346),
            (46.4900, 8.2700, 1390), (46.4700, 8.2450, 1315), (46.4550, 8.2200, 1280),
            (46.4300, 8.1800, 1180), (46.4000, 8.1350, 1050), (46.3900, 8.1150, 1040),
            (46.3760, 8.0850, 900), (46.3560, 8.0450, 760), (46.3380, 8.0150, 700),
            (46.3190, 7.9880, 680),
        ]
        return MeasuredLine(
            coordinates: points.map { Coordinate(latitude: $0.0, longitude: $0.1) },
            elevations: points.map { $0.2 }
        )
    }()

    /// The same line with about 50,000 vertices and sub-hysteresis elevation noise: the load
    /// the frame-rate check drags.
    static let alpsDense: MeasuredLine = {
        let vertices = alps.vertices
        let perSegment = 50_000 / (vertices.count - 1)
        var coordinates: [Coordinate] = []
        var elevations: [Double?] = []
        for (a, b) in zip(vertices, vertices.dropFirst()) {
            for step in 0..<perSegment {
                let t = Double(step) / Double(perSegment)
                coordinates.append(Coordinate(
                    latitude: a.coordinate.latitude + (b.coordinate.latitude - a.coordinate.latitude) * t,
                    longitude: a.coordinate.longitude + (b.coordinate.longitude - a.coordinate.longitude) * t
                ))
                elevations.append(a.elevation + (b.elevation - a.elevation) * t + sin(Double(step) * 0.7))
            }
        }
        coordinates.append(vertices[vertices.count - 1].coordinate)
        elevations.append(vertices[vertices.count - 1].elevation)
        return MeasuredLine(coordinates: coordinates, elevations: elevations)
    }()
}
#endif
