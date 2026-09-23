#if canImport(UIKit) && canImport(MapKit)
import SwiftUI
import MapKit
import OBCDomain

/// The map half of `LineMarkerEditor`: the line in segment colours, and a handle on every
/// marker that drags along the line. UIKit, because a SwiftUI `Map` cannot move an
/// annotation along a line at frame rate.
struct LineMarkerMapView: UIViewRepresentable {
    let model: LineMarkerEditorModel
    let lineVersion: Int
    let markers: [LineMarker]
    let activeID: LineMarker.ID?
    let segmentColors: [Color]
    let dashedSegments: Set<Int>
    let stops: [PlacedStop]

    func makeUIView(context: Context) -> MKMapView {
        let mapView = MKMapView()
        mapView.delegate = context.coordinator
        mapView.isRotateEnabled = false
        mapView.isPitchEnabled = false
        mapView.showsCompass = false
        mapView.preferredConfiguration = MKStandardMapConfiguration(elevationStyle: .flat, emphasisStyle: .muted)
        // Light tiles always: the palette is light throughout.
        mapView.overrideUserInterfaceStyle = .light
        context.coordinator.mapView = mapView
        context.coordinator.install(line: model.line, version: lineVersion, in: mapView, fit: true)
        updateUIView(mapView, context: context)
        return mapView
    }

    func updateUIView(_ mapView: MKMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        if coordinator.lineVersion != lineVersion {
            coordinator.install(line: model.line, version: lineVersion, in: mapView, fit: false)
        }
        coordinator.renderer?.set(
            splits: markers.map(\.distance),
            colors: segmentColors.map { UIColor($0).cgColor },
            dashed: dashedSegments
        )
        if coordinator.stops != stops {
            mapView.removeAnnotations(mapView.annotations.filter { $0 is StopAnnotation })
            mapView.addAnnotations(stops.map(StopAnnotation.init))
            coordinator.stops = stops
        }
        // Diff the annotations by marker id: a marker added, removed or replaced from outside
        // gets its handle without touching the others.
        var present: [LineMarker.ID: MarkerAnnotation] = [:]
        for case let annotation as MarkerAnnotation in mapView.annotations {
            present[annotation.id] = annotation
        }
        let wanted = Set(markers.map(\.id))
        mapView.removeAnnotations(present.values.filter { !wanted.contains($0.id) })
        for marker in markers {
            if let annotation = present[marker.id] {
                if annotation.distance != marker.distance {
                    annotation.distance = marker.distance
                    annotation.coordinate = clLocation(model.line.coordinate(at: marker.distance))
                }
                if let view = mapView.view(for: annotation) as? MarkerAnnotationView {
                    coordinator.configure(view, for: annotation)
                }
            } else {
                mapView.addAnnotation(MarkerAnnotation(
                    id: marker.id, distance: marker.distance, coordinate: model.line.coordinate(at: marker.distance)
                ))
            }
        }
    }

    /// The map going away mid-drag (an offline flip) ends the drag like a lifted finger.
    static func dismantleUIView(_ mapView: MKMapView, coordinator: Coordinator) {
        coordinator.releaseDrag()
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    @MainActor
    final class Coordinator: NSObject, MKMapViewDelegate {
        var parent: LineMarkerMapView
        weak var mapView: MKMapView?
        var renderer: SegmentedLineRenderer?
        private(set) var lineVersion = -1
        /// The stops the map shows pins for.
        var stops: [PlacedStop] = []
        /// The finger on the map, or none: a second finger is ignored until this one lets go.
        private var drag: Drag?

        private struct Drag {
            let annotation: MarkerAnnotation
            /// Set on the first movement, which picks between coincident handles.
            var id: LineMarker.ID?
            /// Where the finger landed relative to the handle's point on the line, so the marker
            /// follows the finger's movement and does not jump under it.
            let grabOffset: CGPoint
            var lastFinger: CGPoint
        }

        init(parent: LineMarkerMapView) {
            self.parent = parent
        }

        /// A new overlay for a new line; the handles are re-added by the annotation diff.
        func install(line: MeasuredLine, version: Int, in mapView: MKMapView, fit: Bool) {
            releaseDrag()
            mapView.removeOverlays(mapView.overlays)
            mapView.removeAnnotations(mapView.annotations)
            stops = []
            renderer = nil
            let overlay = SegmentedLineOverlay(line: line)
            mapView.addOverlay(overlay, level: .aboveRoads)
            lineVersion = version
            if fit {
                // A padded rect, not edge padding: the view has no size yet, and MapKit fits a
                // rect to the final bounds on its own.
                let bounds = overlay.boundingMapRect
                mapView.setVisibleMapRect(
                    bounds.insetBy(dx: -bounds.width * 0.18, dy: -bounds.height * 0.3), animated: false
                )
            }
        }

        func mapView(_ mapView: MKMapView, rendererFor overlay: any MKOverlay) -> MKOverlayRenderer {
            guard let overlay = overlay as? SegmentedLineOverlay else { return MKOverlayRenderer(overlay: overlay) }
            let renderer = SegmentedLineRenderer(overlay: overlay)
            renderer.set(
                splits: parent.markers.map(\.distance),
                colors: parent.segmentColors.map { UIColor($0).cgColor },
                dashed: parent.dashedSegments
            )
            self.renderer = renderer
            return renderer
        }

        func mapView(_ mapView: MKMapView, viewFor annotation: any MKAnnotation) -> MKAnnotationView? {
            if let stop = annotation as? StopAnnotation {
                let view = mapView.dequeueReusableAnnotationView(withIdentifier: "stop") as? StopAnnotationView
                    ?? StopAnnotationView(annotation: stop, reuseIdentifier: "stop")
                view.annotation = stop
                view.configure(kind: stop.kind)
                return view
            }
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
            guard let mapView else { return }
            let finger = touch.location(in: mapView)
            switch phase {
            case .began:
                guard drag == nil, parent.model.activeID == nil else { return }
                let anchor = mapView.convert(annotation.coordinate, toPointTo: mapView)
                drag = Drag(
                    annotation: annotation, id: nil,
                    grabOffset: CGPoint(x: finger.x - anchor.x, y: finger.y - anchor.y), lastFinger: finger
                )
                mapView.isScrollEnabled = false
                mapView.isZoomEnabled = false
            case .moved:
                guard var current = drag, current.annotation === annotation else { return }
                let metersPerPoint = mapView.region.span.latitudeDelta * 111_320 / Double(mapView.bounds.height)
                let travel = hypot(finger.x - current.lastFinger.x, finger.y - current.lastFinger.y) * metersPerPoint
                let window = LineMarkerEditorModel.mapWindow(travelMeters: travel)
                current.lastFinger = finger
                let point = CGPoint(x: finger.x - current.grabOffset.x, y: finger.y - current.grabOffset.y)
                let target = mapView.convert(point, toCoordinateFrom: mapView)
                let coordinate = Coordinate(latitude: target.latitude, longitude: target.longitude)
                if current.id == nil {
                    guard let id = pick(under: annotation, in: mapView, toward: coordinate, window: window),
                        parent.model.begin(id)
                    else { return }
                    current.id = id
                    (mapView.view(for: held(id, in: mapView) ?? annotation) as? MarkerAnnotationView)?.setLifted(true)
                }
                drag = current
                guard let id = current.id, let held = held(id, in: mapView) else { return }
                parent.model.move(id, toward: coordinate, window: window)
                if let distance = parent.model.marker(id)?.distance, distance != held.distance {
                    held.distance = distance
                    held.coordinate = clLocation(parent.model.line.coordinate(at: distance))
                }
            case .ended:
                guard let current = drag, current.annotation === annotation else { return }
                releaseDrag()
            }
        }

        /// The marker under the finger among handles that share this handle's map point.
        private func pick(
            under annotation: MarkerAnnotation, in mapView: MKMapView, toward coordinate: Coordinate, window: Double
        ) -> LineMarker.ID? {
            let anchor = mapView.convert(annotation.coordinate, toPointTo: mapView)
            let coincident = mapView.annotations.compactMap { candidate -> LineMarker.ID? in
                guard let candidate = candidate as? MarkerAnnotation else { return nil }
                let point = mapView.convert(candidate.coordinate, toPointTo: mapView)
                return hypot(point.x - anchor.x, point.y - anchor.y) <= MarkerAnnotationView.reach ? candidate.id : nil
            }
            return parent.model.grab(among: coincident, toward: coordinate, window: window)
        }

        private func held(_ id: LineMarker.ID, in mapView: MKMapView) -> MarkerAnnotation? {
            mapView.annotations.first { ($0 as? MarkerAnnotation)?.id == id } as? MarkerAnnotation
        }

        /// Let go, whether the finger lifted, the touch was cancelled or the map is going away.
        func releaseDrag() {
            guard let current = drag else { return }
            drag = nil
            if let mapView {
                mapView.isScrollEnabled = true
                mapView.isZoomEnabled = true
                if let id = current.id, let held = held(id, in: mapView) {
                    (mapView.view(for: held) as? MarkerAnnotationView)?.setLifted(false)
                }
            }
            if current.id != nil { parent.model.end() }
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
    /// Half the grab area around the pin's tip.
    static let reach: CGFloat = 22

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
        // The hosted view only draws; touches reach this view's `touches*` overrides.
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
        hypot(point.x - anchor.x, point.y - anchor.y) <= Self.reach
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

/// A known stop near the line.
final class StopAnnotation: NSObject, MKAnnotation {
    let kind: Stop.Kind
    let coordinate: CLLocationCoordinate2D

    init(_ placed: PlacedStop) {
        kind = placed.stop.kind
        coordinate = clLocation(placed.stop.coordinate)
    }
}

/// A small round pin for a stop. It takes no touches, so it never steals a handle's drag, and
/// it sits under the handles.
final class StopAnnotationView: MKAnnotationView {
    private static let size: CGFloat = 20
    private let host = UIHostingController(rootView: AnyView(EmptyView()))

    override init(annotation: (any MKAnnotation)?, reuseIdentifier: String?) {
        super.init(annotation: annotation, reuseIdentifier: reuseIdentifier)
        bounds = CGRect(x: 0, y: 0, width: Self.size, height: Self.size)
        host.view.backgroundColor = .clear
        host.view.frame = bounds
        addSubview(host.view)
        isUserInteractionEnabled = false
        // Below required, MapKit hides a pin that meets a handle's wide label frame.
        displayPriority = .required
        zPriority = .min
    }

    required init?(coder: NSCoder) { nil }

    func configure(kind: Stop.Kind) {
        host.rootView = AnyView(
            StopIcon(kind: kind, size: Self.size - 2, isRound: true)
                .overlay(Circle().strokeBorder(OBCTheme.panel, lineWidth: 1.5))
                .frame(width: Self.size, height: Self.size)
        )
    }
}

// MARK: The line

/// The whole line as one overlay. The renderer splits it at the marker distances, so a
/// drag changes two colour runs and never rebuilds a polyline.
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
            if (i + 1) % Self.chunkSize == 0 {
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
    private var dashed: Set<Int> = []
    private let halo = UIColor(OBCTheme.trackHalo).cgColor

    /// Main thread in, `draw` on MapKit's threads out: the lock is the hand-over. A moved
    /// split redraws only the tiles between its old and new place.
    func set(splits: [Double], colors: [CGColor], dashed: Set<Int>) {
        lock.lock()
        let previous = self.splits
        let colorsChanged = colors != self.colors || dashed != self.dashed
        self.splits = splits
        self.colors = colors
        self.dashed = dashed
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
        let dashed = self.dashed
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
        for (run, path) in paths.enumerated() where !dashed.contains(run) {
            context.addPath(path)
        }
        context.strokePath()

        context.setLineWidth(width)
        for (run, path) in paths.enumerated() {
            context.setStrokeColor(run < colors.count ? colors[run] : halo)
            // The dash phase is in map points, so the dashes stay put while a split moves.
            context.setLineDash(phase: 0, lengths: dashed.contains(run) ? [6 / zoomScale, 8 / zoomScale] : [])
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

        var i = first + 1
        while i <= last {
            let chunk = (i - 1) / SegmentedLineOverlay.chunkSize
            let chunkLast = min((chunk + 1) * SegmentedLineOverlay.chunkSize, last)
            if !overlay.chunkRects[chunk].intersects(clip) {
                // Nothing of this chunk shows: lift the pen and skip to its last vertex.
                penDown = false
                previous = overlay.mapPoints[chunkLast]
                i = chunkLast + 1
                continue
            }
            visit(overlay.mapPoints[i], gapBefore: overlay.pieceStarts.contains(i), isLast: false)
            i += 1
        }
        visit(overlay.mapPoint(at: to), gapBefore: false, isLast: true)
        return path
    }
}
#endif
