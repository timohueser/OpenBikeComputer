#if canImport(UIKit) && canImport(MapKit)
import SwiftUI
import MapKit
import OBCDomain

/// The map half of `LineMarkerEditor`: the line in segment colours, and a handle on every
/// marker that drags along the line. UIKit, because a SwiftUI `Map` cannot move an
/// annotation along a line at frame rate.
struct LineMarkerMapView: UIViewRepresentable {
    let model: LineMarkerEditorModel
    let markers: [LineMarker]
    let activeID: LineMarker.ID?
    let segmentColors: [Color]

    /// The projection window, in screen points along the line: a finger can move the marker
    /// at most this far per frame, so a sweep across a switchback never steals it.
    private static let windowPoints = 200.0

    func makeUIView(context: Context) -> MKMapView {
        let mapView = MKMapView()
        mapView.delegate = context.coordinator
        mapView.isRotateEnabled = false
        mapView.isPitchEnabled = false
        mapView.showsCompass = false
        mapView.preferredConfiguration = MKStandardMapConfiguration(elevationStyle: .flat, emphasisStyle: .muted)
        // Light tiles always: the palette is light throughout.
        mapView.overrideUserInterfaceStyle = .light

        let overlay = SegmentedLineOverlay(line: model.line)
        mapView.addOverlay(overlay, level: .aboveRoads)
        // A padded rect, not edge padding: the view has no size yet, and MapKit fits a rect
        // to the final bounds on its own.
        let bounds = overlay.boundingMapRect
        mapView.setVisibleMapRect(bounds.insetBy(dx: -bounds.width * 0.18, dy: -bounds.height * 0.3), animated: false)
        mapView.addAnnotations(markers.map { marker in
            MarkerAnnotation(id: marker.id, distance: marker.distance, coordinate: model.line.coordinate(at: marker.distance))
        })
        context.coordinator.mapView = mapView
        return mapView
    }

    func updateUIView(_ mapView: MKMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        coordinator.renderer?.set(
            splits: markers.map(\.distance),
            colors: segmentColors.map { UIColor($0).cgColor }
        )
        for case let annotation as MarkerAnnotation in mapView.annotations {
            guard let marker = markers.first(where: { $0.id == annotation.id }) else { continue }
            if annotation.distance != marker.distance {
                annotation.distance = marker.distance
                annotation.coordinate = clLocation(model.line.coordinate(at: marker.distance))
            }
            if let view = mapView.view(for: annotation) as? MarkerAnnotationView {
                coordinator.configure(view, for: annotation)
            }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    @MainActor
    final class Coordinator: NSObject, MKMapViewDelegate {
        var parent: LineMarkerMapView
        weak var mapView: MKMapView?
        var renderer: SegmentedLineRenderer?
        /// Where the finger landed relative to the handle's point on the line, so the marker
        /// follows the finger's movement and does not jump under it.
        private var grabOffset = CGPoint.zero

        init(parent: LineMarkerMapView) {
            self.parent = parent
        }

        func mapView(_ mapView: MKMapView, rendererFor overlay: any MKOverlay) -> MKOverlayRenderer {
            guard let overlay = overlay as? SegmentedLineOverlay else { return MKOverlayRenderer(overlay: overlay) }
            let renderer = SegmentedLineRenderer(overlay: overlay)
            renderer.set(
                splits: parent.markers.map(\.distance),
                colors: parent.segmentColors.map { UIColor($0).cgColor }
            )
            self.renderer = renderer
            return renderer
        }

        func mapView(_ mapView: MKMapView, viewFor annotation: any MKAnnotation) -> MKAnnotationView? {
            guard let annotation = annotation as? MarkerAnnotation else { return nil }
            let identifier = "marker"
            let view = mapView.dequeueReusableAnnotationView(withIdentifier: identifier) as? MarkerAnnotationView
                ?? MarkerAnnotationView(annotation: annotation, reuseIdentifier: identifier)
            view.annotation = annotation
            view.onDrag = { [weak self] phase, touch in self?.drag(phase, touch: touch, annotation: annotation) }
            configure(view, for: annotation)
            return view
        }

        func configure(_ view: MarkerAnnotationView, for annotation: MarkerAnnotation) {
            let isActive = parent.activeID == annotation.id
            view.configure(
                color: parent.model.color(endingAt: annotation.id),
                isActive: isActive,
                label: isActive ? parent.model.label(for: annotation.id) : nil
            )
        }

        private func drag(_ phase: MarkerAnnotationView.DragPhase, touch: UITouch, annotation: MarkerAnnotation) {
            guard let mapView, let view = mapView.view(for: annotation) as? MarkerAnnotationView else { return }
            let model = parent.model
            let finger = touch.location(in: mapView)
            switch phase {
            case .began:
                let anchor = mapView.convert(annotation.coordinate, toPointTo: mapView)
                grabOffset = CGPoint(x: finger.x - anchor.x, y: finger.y - anchor.y)
                mapView.isScrollEnabled = false
                mapView.isZoomEnabled = false
                view.setLifted(true)
                model.begin(annotation.id)
            case .moved:
                let point = CGPoint(x: finger.x - grabOffset.x, y: finger.y - grabOffset.y)
                let coordinate = mapView.convert(point, toCoordinateFrom: mapView)
                let metersPerPoint = mapView.region.span.latitudeDelta * 111_320 / Double(mapView.bounds.height)
                model.move(
                    annotation.id,
                    toward: Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude),
                    window: LineMarkerMapView.windowPoints * metersPerPoint
                )
                if let distance = model.marker(annotation.id)?.distance, distance != annotation.distance {
                    annotation.distance = distance
                    annotation.coordinate = clLocation(model.line.coordinate(at: distance))
                }
            case .ended:
                mapView.isScrollEnabled = true
                mapView.isZoomEnabled = true
                view.setLifted(false)
                model.end()
            }
        }
    }
}

private func clLocation(_ coordinate: Coordinate) -> CLLocationCoordinate2D {
    CLLocationCoordinate2D(latitude: coordinate.latitude, longitude: coordinate.longitude)
}

// MARK: Annotations

final class MarkerAnnotation: NSObject, MKAnnotation {
    let id: LineMarker.ID
    var distance: Double
    @objc dynamic var coordinate: CLLocationCoordinate2D

    init(id: LineMarker.ID, distance: Double, coordinate: Coordinate) {
        self.id = id
        self.distance = distance
        self.coordinate = clLocation(coordinate)
    }
}

/// The handle on the map: the same SwiftUI handle as the profile, hosted, with its label
/// above it. Only the handle itself takes touches, so two nearby markers do not overlap.
final class MarkerAnnotationView: MKAnnotationView {
    /// Wide enough for the label, tall enough for the label over the pin.
    private static let hostSize = CGSize(width: 180, height: 64)

    enum DragPhase { case began, moved, ended }

    private let host = UIHostingController(rootView: AnyView(EmptyView()))
    private var anchor = CGPoint.zero
    /// Raw touches, not a pan recognizer: MapKit's own recognizers on the map view win the
    /// arbitration against a recognizer on an annotation view, and the touch goes nowhere.
    var onDrag: ((DragPhase, UITouch) -> Void)?

    override init(annotation: (any MKAnnotation)?, reuseIdentifier: String?) {
        super.init(annotation: annotation, reuseIdentifier: reuseIdentifier)
        bounds = CGRect(origin: .zero, size: Self.hostSize)
        host.view.backgroundColor = .clear
        // The hosted view only draws; touches reach this view and its pan recognizer.
        host.view.isUserInteractionEnabled = false
        host.view.frame = bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(host.view)
        displayPriority = .required
        isAccessibilityElement = false
    }

    required init?(coder: NSCoder) { nil }

    func configure(color: Color, isActive: Bool, label: String?) {
        host.rootView = AnyView(
            VStack(spacing: 4) {
                if let label { MarkerLabel(text: label).fixedSize() }
                MarkerHandleView(color: color, isActive: isActive)
            }
            .frame(width: Self.hostSize.width, height: Self.hostSize.height, alignment: .bottom)
        )
        // The pin's tip is the bottom centre of the hosted view.
        anchor = CGPoint(x: Self.hostSize.width / 2, y: Self.hostSize.height)
        centerOffset = CGPoint(x: 0, y: Self.hostSize.height / 2 - anchor.y)
        zPriority = isActive ? .max : .defaultSelected
    }

    func setLifted(_ lifted: Bool) {
        UIView.animate(withDuration: 0.16, delay: 0, options: [.curveEaseOut]) {
            self.transform = lifted ? CGAffineTransform(translationX: 0, y: -MarkerHandleView.dragLift) : .identity
        }
    }

    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        hypot(point.x - anchor.x, point.y - anchor.y) <= 22
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let touch = touches.first { onDrag?(.began, touch) }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let touch = touches.first { onDrag?(.moved, touch) }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let touch = touches.first { onDrag?(.ended, touch) }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        if let touch = touches.first { onDrag?(.ended, touch) }
    }
}

// MARK: The line

/// The whole line as one overlay. The renderer splits it at the marker distances, so a
/// drag changes two colour runs and never rebuilds a polyline.
final class SegmentedLineOverlay: NSObject, MKOverlay {
    let line: MeasuredLine
    let mapPoints: [MKMapPoint]
    let pieceStarts: Set<Int>
    let boundingMapRect: MKMapRect

    init(line: MeasuredLine) {
        self.line = line
        let points = line.vertices.map { MKMapPoint(clLocation($0.coordinate)) }
        mapPoints = points
        pieceStarts = Set(line.pieceStarts)
        var rect = MKMapRect.null
        for point in points {
            rect = rect.union(MKMapRect(origin: point, size: MKMapSize(width: 0, height: 0)))
        }
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

/// Draws the line in colour runs between the splits, simplified to the tile's zoom and
/// culled to the tile, so a 50,000-point line costs what is visible.
final class SegmentedLineRenderer: MKOverlayRenderer {
    private let lock = NSLock()
    private var splits: [Double] = []
    private var colors: [CGColor] = []
    private let halo = UIColor(OBCTheme.trackHalo).cgColor

    /// Main thread in, `draw` on MapKit's threads out: the lock is the hand-over. A moved
    /// split redraws only the tiles between its old and new place.
    func set(splits: [Double], colors: [CGColor]) {
        lock.lock()
        let previous = self.splits
        let colorsChanged = colors != self.colors
        self.splits = splits
        self.colors = colors
        lock.unlock()
        guard let overlay = overlay as? SegmentedLineOverlay else { return }
        if colorsChanged || previous.count != splits.count {
            setNeedsDisplay()
            return
        }
        for (old, new) in zip(previous, splits) where old != new {
            setNeedsDisplay(overlay.rect(between: old, and: new))
        }
    }

    override func draw(_ mapRect: MKMapRect, zoomScale: MKZoomScale, in context: CGContext) {
        guard let overlay = overlay as? SegmentedLineOverlay, overlay.mapPoints.count > 1 else { return }
        lock.lock()
        let splits = self.splits
        let colors = self.colors
        lock.unlock()

        let width = 3.4 / zoomScale
        let haloWidth = 7 / zoomScale
        let clip = mapRect.insetBy(dx: -haloWidth * 2, dy: -haloWidth * 2)
        // Below a screen pixel, a vertex adds nothing.
        let tolerance = 0.75 / Double(zoomScale)
        let bounds = [0] + splits + [overlay.line.length]
        let paths = (0..<(bounds.count - 1)).map { run in
            path(of: overlay, from: bounds[run], to: bounds[run + 1], clip: clip, tolerance: tolerance)
        }

        context.setLineCap(.round)
        context.setLineJoin(.round)
        context.setLineWidth(haloWidth)
        context.setStrokeColor(halo)
        for path in paths {
            context.addPath(path)
        }
        context.strokePath()

        context.setLineWidth(width)
        for (run, path) in paths.enumerated() {
            context.setStrokeColor(run < colors.count ? colors[run] : halo)
            context.addPath(path)
            context.strokePath()
        }
    }

    private func path(
        of overlay: SegmentedLineOverlay, from: Double, to: Double, clip: MKMapRect, tolerance: Double
    ) -> CGPath {
        let path = CGMutablePath()
        guard to > from else { return path }
        let first = overlay.line.index(at: from)
        let last = overlay.line.index(at: to)
        var previous = overlay.mapPoint(at: from)
        var emitted = previous
        var penDown = false

        func visit(_ point: MKMapPoint, gapBefore: Bool, isLast: Bool) {
            defer { previous = point }
            if gapBefore {
                penDown = false
                return
            }
            let box = MKMapRect(
                x: min(previous.x, point.x), y: min(previous.y, point.y),
                width: abs(point.x - previous.x), height: abs(point.y - previous.y)
            )
            guard box.intersects(clip) else {
                penDown = false
                return
            }
            if !penDown {
                path.move(to: self.point(for: previous))
                emitted = previous
                penDown = true
            }
            if !isLast, abs(point.x - emitted.x) + abs(point.y - emitted.y) < tolerance { return }
            path.addLine(to: self.point(for: point))
            emitted = point
        }

        if first + 1 <= last {
            for i in (first + 1)...last {
                visit(overlay.mapPoints[i], gapBefore: overlay.pieceStarts.contains(i), isLast: false)
            }
        }
        visit(overlay.mapPoint(at: to), gapBefore: false, isLast: true)
        return path
    }
}
#endif
