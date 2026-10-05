#if os(iOS)
import MapLibre
import MapKit
import OBCDomain
import SwiftUI

struct LineMarkerMapView: UIViewRepresentable {
    let model: LineMarkerEditorModel
    let markers: [LineMarker]
    let activeID: LineMarker.ID?
    let segmentColors: [Color]
    let dashedSegments: Set<Int>
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.obcPlannerSource) private var plannerSource

    func makeUIView(context: Context) -> OBCNativeMapView {
        let map = OBCNativeMapView()
        map.delegate = context.coordinator
        context.coordinator.map = map
        map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
            guard let map, let coordinator else { return }
            map.fit(coordinator.parent.model.line.vertices.map(\.coordinate))
        }
        return map
    }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func updateUIView(_ map: OBCNativeMapView, context: Context) {
        let c = context.coordinator
        c.parent = self
        map.load(dark: colorScheme == .dark, source: plannerSource)
        if activeID == nil { c.draw(map) }
        c.updatePins(map)
    }
    static func dismantleUIView(_ map: OBCNativeMapView, coordinator: Coordinator) {
        coordinator.release()
        map.stop()
    }

    @MainActor final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var parent: LineMarkerMapView
        weak var map: OBCNativeMapView?
        private var drag: (annotation: MarkerAnnotation, id: LineMarker.ID?, offset: CGPoint, finger: CGPoint)?
        init(_ parent: LineMarkerMapView) { self.parent = parent }

        func draw(_ map: OBCNativeMapView, force: Bool = false) {
            let line = parent.model.line, colors = parent.segmentColors, dashed = parent.dashedSegments
            let bounds = [0] + parent.markers.map(\.distance) + [line.length]
            var strokes: [MapStroke] = []
            for index in colors.indices {
                let from = bounds[index], to = bounds[index + 1]
                guard to > from else { continue }
                let first = line.index(at: from), last = line.index(at: to)
                var piece = [line.coordinate(at: from)]
                if first < last {
                    for i in (first + 1)...last {
                        if line.pieceStarts.contains(i) {
                            strokes.append(MapStroke(coordinates: piece, color: colors[index], cased: parent.model.cased && !dashed.contains(index), dash: dashed.contains(index) ? [2, 2.5] : []))
                            piece = []
                        }
                        piece.append(line.vertices[i].coordinate)
                    }
                }
                piece.append(line.coordinate(at: to))
                strokes.append(MapStroke(coordinates: piece, color: colors[index], cased: parent.model.cased && !dashed.contains(index), dash: dashed.contains(index) ? [2, 2.5] : []))
            }
            map.draw(strokes, force: force)
        }
        func updatePins(_ map: OBCNativeMapView) {
            let present = (map.annotations ?? []).compactMap { $0 as? MarkerAnnotation }
            let wanted = Set(parent.markers.map(\.id))
            map.removeAnnotations(present.filter { !wanted.contains($0.id) })
            for marker in parent.markers {
                if let annotation = present.first(where: { $0.id == marker.id }) {
                    annotation.coordinate = clLocation(parent.model.line.coordinate(at: marker.distance))
                    annotation.distance = marker.distance
                    annotation.title = marker.name
                    if let view = map.view(for: annotation) as? HandleView { configure(view, annotation: annotation) }
                } else { map.addAnnotation(MarkerAnnotation(marker, coordinate: clLocation(parent.model.line.coordinate(at: marker.distance)))) }
            }
        }
        func mapViewDidFinishLoadingMap(_ mapView: MLNMapView) {
            (mapView as? OBCNativeMapView)?.didFinishLoadingMap()
        }
        func mapViewDidFailLoadingMap(_ mapView: MLNMapView, withError error: Error) {
            (mapView as? OBCNativeMapView)?.didFailLoadingMap()
        }
        func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
            if let map = mapView as? OBCNativeMapView { draw(map, force: true) }
        }
        func mapView(_ mapView: MLNMapView, regionDidChangeAnimated animated: Bool) {
            (mapView as? OBCNativeMapView)?.updateCoverageStatus()
            if let map = mapView as? OBCNativeMapView { updatePins(map) }
        }
        func mapView(_ mapView: MLNMapView, viewFor annotation: any MLNAnnotation) -> MLNAnnotationView? {
            guard let annotation = annotation as? MarkerAnnotation else { return nil }
            let view = HandleView(reuseIdentifier: "handle")
            view.annotation = annotation
            view.onTouch = { [weak self, weak annotation] phase, touch in
                guard let annotation else { return }; self?.touch(phase, touch: touch, annotation: annotation)
            }
            configure(view, annotation: annotation)
            return view
        }
        private func configure(_ view: HandleView, annotation: MarkerAnnotation) {
            let active = parent.activeID == annotation.id
            view.host.rootView = AnyView(VStack(spacing: 4) {
                if active { MarkerLabel(text: parent.model.label(for: annotation.id)).fixedSize() }
                MarkerHandleView(color: parent.model.color(endingAt: annotation.id), isActive: active,
                                 isFixed: parent.model.marker(annotation.id)?.isFixed ?? false)
            }.frame(width: 180, height: 64, alignment: .bottom))
            view.setLifted(active)
        }
        private func touch(_ phase: HandleView.Phase, touch: UITouch, annotation: MarkerAnnotation) {
            guard let map else { return }
            let finger = touch.location(in: map)
            switch phase {
            case .began:
                guard drag == nil, parent.model.activeID == nil else { return }
                let anchor = map.convert(annotation.coordinate, toPointTo: map)
                drag = (annotation, nil, CGPoint(x: finger.x - anchor.x, y: finger.y - anchor.y), finger)
                map.isScrollEnabled = false; map.isZoomEnabled = false
            case .moved:
                guard var current = drag, current.annotation === annotation else { return }
                let travel = hypot(finger.x - current.finger.x, finger.y - current.finger.y) * map.metersPerPoint(atLatitude: map.centerCoordinate.latitude)
                let window = LineMarkerEditorModel.mapWindow(travelMeters: travel)
                let coordinate = map.convert(CGPoint(x: finger.x - current.offset.x, y: finger.y - current.offset.y), toCoordinateFrom: map)
                let target = Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude)
                if current.id == nil {
                    let anchor = map.convert(annotation.coordinate, toPointTo: map)
                    let ids = (map.annotations ?? []).compactMap { pin -> LineMarker.ID? in
                        guard let pin = pin as? MarkerAnnotation else { return nil }
                        let point = map.convert(pin.coordinate, toPointTo: map)
                        return hypot(point.x - anchor.x, point.y - anchor.y) <= 22 ? pin.id : nil
                    }
                    guard let id = parent.model.grab(among: ids, toward: target, window: window), parent.model.begin(id) else { return }
                    current.id = id
                }
                current.finger = finger; drag = current
                if let id = current.id {
                    parent.model.move(id, toward: target, window: window)
                    if let pin = (map.annotations ?? []).first(where: { ($0 as? MarkerAnnotation)?.id == id }) as? MarkerAnnotation,
                       let marker = parent.model.marker(id) { pin.coordinate = clLocation(parent.model.line.coordinate(at: marker.distance)) }
                }
            case .ended:
                guard let current = drag, current.annotation === annotation else { return }
                release()
                if current.id == nil { parent.model.tap(annotation.id) }
            }
        }
        func release() {
            guard let current = drag else { return }
            drag = nil
            map?.isScrollEnabled = true; map?.isZoomEnabled = true
            if current.id != nil { parent.model.end() }
        }
    }
}

private func clLocation(_ coordinate: Coordinate) -> CLLocationCoordinate2D { MapGeometry.clLocation(coordinate) }
private final class MarkerAnnotation: MLNPointAnnotation {
    let id: LineMarker.ID
    var distance: Double
    init(_ marker: LineMarker, coordinate: CLLocationCoordinate2D) {
        id = marker.id; distance = marker.distance
        super.init(); self.coordinate = coordinate; title = marker.name
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
}
private final class HandleView: MLNAnnotationView {
    enum Phase { case began, moved, ended }
    let host = UIHostingController(rootView: AnyView(EmptyView()))
    var onTouch: ((Phase, UITouch) -> Void)?
    override init(reuseIdentifier: String?) {
        super.init(reuseIdentifier: reuseIdentifier)
        frame.size = CGSize(width: 180, height: 64)
        centerOffset = CGVector(dx: 0, dy: -32)
        host.view.frame = bounds
        host.view.backgroundColor = .clear
        host.view.isUserInteractionEnabled = false
        addSubview(host.view)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        hypot(point.x - bounds.midX, point.y - bounds.maxY) <= 22
    }
    func setLifted(_ lifted: Bool) {
        let target = lifted ? CGAffineTransform(translationX: 0, y: -MarkerHandleView.dragLift) : .identity
        guard transform != target else { return }
        UIView.animate(withDuration: 0.16, delay: 0, options: [.curveEaseOut]) { self.transform = target }
    }
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) { if let touch = touches.first { onTouch?(.began, touch) } }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) { if let touch = touches.first { onTouch?(.moved, touch) } }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { if let touch = touches.first { onTouch?(.ended, touch) } }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { if let touch = touches.first { onTouch?(.ended, touch) } }
}

// Projected line chunks support the linked elevation profile without scanning invisible segments.
final class SegmentedLineOverlay: NSObject, MKOverlay {
    /// Vertices per chunk of the bounds index.
    static let chunkSize = 256

    let line: MeasuredLine
    let mapPoints: [MKMapPoint]
    let pieceStarts: Set<Int>
    let boundingMapRect: MKMapRect
    /// The bounds of each chunk of `chunkSize` segments, so a tile walks only the chunks it
    /// touches. Chunk `k` covers the segments from vertex `k * chunkSize` to the next chunk's
    /// first vertex.
    let chunkRects: [MKMapRect]

    init(line: MeasuredLine) {
        self.line = line
        let points = line.vertices.map { MKMapPoint(clLocation($0.coordinate)) }
        mapPoints = points
        pieceStarts = line.pieceStarts
        var rect = MKMapRect.null
        var chunks: [MKMapRect] = []
        var chunk = MKMapRect.null
        for (i, point) in points.enumerated() {
            let dot = MKMapRect(origin: point, size: MKMapSize(width: 0, height: 0))
            rect = rect.union(dot)
            chunk = chunk.union(dot)
            if i > 0, i % Self.chunkSize == 0 {
                chunks.append(chunk)
                // The chunk's last vertex also starts the next chunk's first segment.
                chunk = dot
            }
        }
        if !chunk.isNull { chunks.append(chunk) }
        chunkRects = chunks
        boundingMapRect = rect.isNull ? MKMapRect.world : rect
    }

    var coordinate: CLLocationCoordinate2D {
        MKMapPoint(x: boundingMapRect.midX, y: boundingMapRect.midY).coordinate
    }

    /// The part of the line between two distances, padded by a twentieth of the line's extent:
    /// wider than any stroke at a zoom where the line fills more than a few dozen pixels.
    func rect(between a: Double, and b: Double) -> MKMapRect {
        let low = line.index(at: min(a, b))
        let high = min(line.index(at: max(a, b)) + 1, mapPoints.count - 1)
        var rect = MKMapRect.null
        for i in low...high {
            rect = rect.union(MKMapRect(origin: mapPoints[i], size: MKMapSize(width: 0, height: 0)))
        }
        let pad = max(boundingMapRect.width, boundingMapRect.height) / 20
        return rect.insetBy(dx: -pad, dy: -pad)
    }

    /// The parts of the line inside `rect` as distance ranges in line order, and the distance of
    /// their point nearest the rect's centre. Clipped per segment, so a long segment across the
    /// rect counts although neither of its ends is inside.
    func pieces(in rect: MKMapRect) -> (pieces: [ClosedRange<Double>], centre: Double) {
        let vertices = line.vertices
        let mid = MKMapPoint(x: rect.midX, y: rect.midY)
        var pieces: [ClosedRange<Double>] = []
        var centre = (distance: 0.0, gap: Double.infinity)
        for chunk in chunkRects.indices where chunkRects[chunk].insetBy(dx: -1, dy: -1).intersects(rect) {
            for i in (chunk * Self.chunkSize)..<min((chunk + 1) * Self.chunkSize, mapPoints.count - 1)
            where !pieceStarts.contains(i + 1) {
                let a = mapPoints[i], b = mapPoints[i + 1]
                let dx = b.x - a.x, dy = b.y - a.y
                // Liang–Barsky: the share of the segment inside the rect is t0...t1.
                var t0 = 0.0, t1 = 1.0
                for (p, q) in [(-dx, a.x - rect.minX), (dx, rect.maxX - a.x), (-dy, a.y - rect.minY), (dy, rect.maxY - a.y)] {
                    if p == 0 {
                        if q < 0 { t0 = 2 }
                    } else if p < 0 {
                        t0 = max(t0, q / p)
                    } else {
                        t1 = min(t1, q / p)
                    }
                }
                guard t0 <= t1 else { continue }
                let da = vertices[i].distance, span = vertices[i + 1].distance - da
                let from = da + span * t0, to = da + span * t1
                if let last = pieces.last, from <= last.upperBound + MeasuredLine.tieMeters {
                    pieces[pieces.count - 1] = last.lowerBound...max(last.upperBound, to)
                } else {
                    pieces.append(from...to)
                }
                let length = dx * dx + dy * dy
                let t = length > 0 ? min(max(((mid.x - a.x) * dx + (mid.y - a.y) * dy) / length, t0), t1) : t0
                let gap = hypot(a.x + dx * t - mid.x, a.y + dy * t - mid.y)
                if gap < centre.gap { centre = (da + span * t, gap) }
            }
        }
        return (pieces, centre.distance)
    }

    /// The map point at a distance along the line, on the segment `MeasuredLine.index` picks.
    func mapPoint(at distance: Double) -> MKMapPoint {
        let i = line.index(at: distance)
        guard i + 1 < mapPoints.count else { return mapPoints[i] }
        let a = line.vertices[i], b = line.vertices[i + 1]
        let span = b.distance - a.distance
        let t = span > 0 ? min(max((distance - a.distance) / span, 0), 1) : 0
        return MKMapPoint(
            x: mapPoints[i].x + (mapPoints[i + 1].x - mapPoints[i].x) * t,
            y: mapPoints[i].y + (mapPoints[i + 1].y - mapPoints[i].y) * t
        )
    }
}


#endif
