#if canImport(UIKit) && canImport(MapKit)
import SwiftUI
import MapKit
import OBCDomain

/// The map half of `LineMarkerEditor`: the line in segment colours, and a handle on every
/// marker that drags along the line. UIKit, because a SwiftUI `Map` cannot move an
/// annotation along a line at frame rate.
///
/// Linked to the profile, the map only looks: its pins take no drag, the profile shows the
/// stretch in view, and stop pins show once the map is close enough.
struct LineMarkerMapView: UIViewRepresentable {
    let model: LineMarkerEditorModel
    let lineVersion: Int
    let markers: [LineMarker]
    let activeID: LineMarker.ID?
    let segmentColors: [Color]
    let dashedSegments: Set<Int>
    let stops: [PlacedStop]
    /// The line fits above this much of the bottom, and the profile shows the line above it:
    /// the part a sheet covers.
    var bottomInset: CGFloat = 0
    var linksProfile = false

    /// Stop pins show only while the map spans less than this: a whole-trip view stays clean.
    static let stopsSpanMeters = 40_000.0

    func makeUIView(context: Context) -> MKMapView {
        let mapView = FittingMapView()
        mapView.onFirstLayout = { [weak coordinator = context.coordinator] in coordinator?.fit() }
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
        // The colour runs follow the markers at rest: a drag moves the handle alone, and the
        // line recolours when the finger lets go, so no tile re-rasterises mid-drag.
        if activeID == nil {
            coordinator.renderer?.set(
                splits: markers.map(\.distance),
                colors: segmentColors.map { UIColor($0).cgColor },
                dashed: dashedSegments
            )
        }
        if coordinator.stops != stops {
            coordinator.stops = stops
            coordinator.showStops(in: mapView)
        }
        // Which day a stop can end moves with the day ends at rest; a drag frame and a sheet
        // resize leave the stop views alone.
        if activeID == nil, coordinator.restingMarkers != model.restingMarkers {
            coordinator.restingMarkers = model.restingMarkers
            for case let stop as StopAnnotation in mapView.annotations {
                (mapView.view(for: stop) as? StopAnnotationView)?.configure(kind: stop.kind, action: model.stopActionTitle(stop.placed))
            }
        }
        if coordinator.bottomInset != bottomInset {
            coordinator.bottomInset = bottomInset
            coordinator.reportVisibleSoon()
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
                    annotation.coordinate = coordinator.coordinate(at: marker.distance)
                }
                if annotation.title != marker.name { annotation.title = marker.name }
                if let view = mapView.view(for: annotation) {
                    coordinator.configure(view, for: annotation)
                }
            } else {
                mapView.addAnnotation(MarkerAnnotation(
                    id: marker.id, distance: marker.distance, coordinate: coordinator.coordinate(at: marker.distance),
                    title: marker.name
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
        /// The known stops; pins for them show while the map is zoomed in enough.
        var stops: [PlacedStop] = []
        private var stopsShown = false
        /// The sheet height the visible stretch was last reported for.
        var bottomInset: CGFloat = 0
        /// The day ends the stop callouts were last set up for.
        var restingMarkers: [LineMarker] = []
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
                // rect to the final bounds on its own. `fit()` does it again with the real
                // bounds and the sheet's inset once the view is laid out.
                let bounds = overlay.boundingMapRect
                mapView.setVisibleMapRect(
                    bounds.insetBy(dx: -bounds.width * 0.18, dy: -bounds.height * 0.3), animated: false
                )
            }
        }

        /// The whole line above the sheet, once the view has its size.
        func fit() {
            guard let mapView, let overlay = mapView.overlays.first as? SegmentedLineOverlay else { return }
            mapView.setVisibleMapRect(
                overlay.boundingMapRect,
                edgePadding: UIEdgeInsets(top: 72, left: 36, bottom: parent.bottomInset + 36, right: 36),
                animated: false)
            reportVisible()
        }

        /// A pin's place: the point the renderer draws at that distance, so the tip is on the
        /// drawn line at every zoom.
        func coordinate(at distance: Double) -> CLLocationCoordinate2D {
            guard let overlay = mapView?.overlays.first as? SegmentedLineOverlay else {
                return clLocation(parent.model.line.coordinate(at: distance))
            }
            return overlay.mapPoint(at: distance).coordinate
        }

        /// Stop pins come and go with the zoom. Only the stops that changed are added or
        /// removed, so an open callout stays open while more stops arrive.
        func showStops(in mapView: MKMapView) {
            let wanted = Self.spanMeters(of: mapView) < LineMarkerMapView.stopsSpanMeters && !stops.isEmpty
            let shown = mapView.annotations.compactMap { $0 as? StopAnnotation }
            let keep = wanted ? Set(stops.map(\.stop)) : []
            mapView.removeAnnotations(shown.filter { !keep.contains($0.placed.stop) })
            let present = Set(shown.map(\.placed.stop))
            mapView.addAnnotations(stops.filter { keep.contains($0.stop) && !present.contains($0.stop) }.map(StopAnnotation.init))
            stopsShown = wanted
        }

        func mapView(_ mapView: MKMapView, regionDidChangeAnimated animated: Bool) {
            let wanted = Self.spanMeters(of: mapView) < LineMarkerMapView.stopsSpanMeters && !stops.isEmpty
            if wanted != stopsShown { showStops(in: mapView) }
            reportVisible()
        }

        /// The sheet moves in many small steps: report once it rests.
        func reportVisibleSoon() {
            NSObject.cancelPreviousPerformRequests(withTarget: self, selector: #selector(reportVisible), object: nil)
            perform(#selector(reportVisible), with: nil, afterDelay: 0.2)
        }

        /// Hand the profile the parts of the line in view above the sheet; close up, ask for
        /// the stops along them.
        @objc func reportVisible() {
            guard parent.linksProfile, let mapView, let overlay = mapView.overlays.first as? SegmentedLineOverlay,
                mapView.bounds.height > parent.bottomInset
            else { return }
            let visible = mapView.visibleMapRect
            let share = Double((mapView.bounds.height - parent.bottomInset) / mapView.bounds.height)
            let above = MKMapRect(x: visible.minX, y: visible.minY, width: visible.width, height: visible.height * share)
            let inView = overlay.pieces(in: above)
            parent.model.showVisible(inView.pieces, centre: inView.centre)
            if Self.spanMeters(of: mapView) < LineMarkerMapView.stopsSpanMeters,
                let first = inView.pieces.first, let last = inView.pieces.last {
                parent.model.onCloseUp(first.lowerBound...last.upperBound)
            }
        }

        /// The width of the visible map in metres.
        private static func spanMeters(of mapView: MKMapView) -> Double {
            mapView.visibleMapRect.width / MKMapPointsPerMeterAtLatitude(mapView.centerCoordinate.latitude)
        }

        /// The callout's one button: the stop's action.
        func mapView(
            _ mapView: MKMapView, annotationView view: MKAnnotationView, calloutAccessoryControlTapped control: UIControl
        ) {
            guard let stop = view.annotation as? StopAnnotation else { return }
            mapView.deselectAnnotation(stop, animated: true)
            parent.model.onStopAction(stop.placed)
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
                view.configure(kind: stop.kind, action: parent.model.stopActionTitle(stop.placed))
                return view
            }
            guard let annotation = annotation as? MarkerAnnotation else { return nil }
            if parent.linksProfile {
                let view = mapView.dequeueReusableAnnotationView(withIdentifier: "pin") as? PinAnnotationView
                    ?? PinAnnotationView(annotation: annotation, reuseIdentifier: "pin")
                view.annotation = annotation
                configure(view, for: annotation)
                return view
            }
            let identifier = "marker"
            let view = mapView.dequeueReusableAnnotationView(withIdentifier: identifier) as? MarkerAnnotationView
                ?? MarkerAnnotationView(annotation: annotation, reuseIdentifier: identifier)
            view.annotation = annotation
            view.onDrag = { [weak self] phase, touch in self?.drag(phase, touch: touch, annotation: annotation) }
            configure(view, for: annotation)
            return view
        }

        func configure(_ view: MKAnnotationView, for annotation: MarkerAnnotation) {
            let isActive = parent.activeID == annotation.id
            let color = parent.model.color(endingAt: annotation.id)
            let isFixed = parent.model.marker(annotation.id)?.isFixed ?? false
            if let view = view as? PinAnnotationView {
                view.configure(color: color, isActive: isActive, isFixed: isFixed)
            } else if let view = view as? MarkerAnnotationView {
                view.configure(
                    color: color, isActive: isActive, isFixed: isFixed,
                    label: isActive ? parent.model.label(for: annotation.id) : nil)
            }
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
                    held.coordinate = self.coordinate(at: distance)
                }
            case .ended:
                guard let current = drag, current.annotation === annotation else { return }
                releaseDrag()
                if current.id == nil { parent.model.tap(annotation.id) }
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
    /// What VoiceOver reads for the pin: "Day 2 end".
    @objc dynamic var title: String?

    init(id: LineMarker.ID, distance: Double, coordinate: CLLocationCoordinate2D, title: String) {
        self.id = id
        self.distance = distance
        self.coordinate = coordinate
        self.title = title
    }
}

/// A pin on a map linked to the profile: the handle drawn once into an image, with the tip at
/// the image's bottom centre. An image, not a hosted SwiftUI view, so nothing but
/// `centerOffset` places the tip. It takes no touch: a drag on it pans the map, and a tap
/// reaches the stop under it.
final class PinAnnotationView: MKAnnotationView {
    private static var images: [String: UIImage] = [:]

    override init(annotation: (any MKAnnotation)?, reuseIdentifier: String?) {
        super.init(annotation: annotation, reuseIdentifier: reuseIdentifier)
        displayPriority = .required
        // Never selected: MapKit then hands a tap to the stop under the pin.
        isEnabled = false
    }

    required init?(coder: NSCoder) { nil }

    func configure(color: Color, isActive: Bool, isFixed: Bool) {
        let key = "\(color)|\(isActive)|\(isFixed)"
        let image = Self.images[key] ?? {
            let renderer = ImageRenderer(content: MarkerHandleView(color: color, isActive: isActive, isFixed: isFixed))
            renderer.scale = UITraitCollection.current.displayScale
            let image = renderer.uiImage ?? UIImage()
            Self.images[key] = image
            return image
        }()
        self.image = image
        centerOffset = CGPoint(x: 0, y: -image.size.height / 2)
        zPriority = isActive ? .max : .defaultSelected
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

    func configure(color: Color, isActive: Bool, isFixed: Bool, label: String?) {
        host.rootView = AnyView(
            VStack(spacing: 4) {
                if let label { MarkerLabel(text: label).fixedSize() }
                MarkerHandleView(color: color, isActive: isActive, isFixed: isFixed)
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

/// A known stop near the line, with the callout text: "Hotel · 290 m off the line".
final class StopAnnotation: NSObject, MKAnnotation {
    let placed: PlacedStop
    let kind: Stop.Kind
    let coordinate: CLLocationCoordinate2D
    let title: String?
    let subtitle: String?

    init(_ placed: PlacedStop) {
        self.placed = placed
        kind = placed.stop.kind
        coordinate = clLocation(placed.stop.coordinate)
        title = placed.stop.name
        subtitle = "\(StopIcon.name(placed.stop.kind)) · \(OBCFormat.stopOffset(meters: placed.offset))"
    }
}

/// An `MKMapView` that says when it first has a size, so the line can be fitted above the sheet.
private final class FittingMapView: MKMapView {
    var onFirstLayout: (() -> Void)?

    override func layoutSubviews() {
        super.layoutSubviews()
        guard bounds.height > 0, let onFirstLayout else { return }
        self.onFirstLayout = nil
        onFirstLayout()
    }
}

/// A small round pin for a stop, under the handles. A tap shows a callout with its name, kind
/// and distance off the line, and the stop's one action as a button when it has one.
final class StopAnnotationView: MKAnnotationView {
    private static let size: CGFloat = 20
    private let host = UIHostingController(rootView: AnyView(EmptyView()))
    private var kind: Stop.Kind?
    private var action: String?

    override init(annotation: (any MKAnnotation)?, reuseIdentifier: String?) {
        super.init(annotation: annotation, reuseIdentifier: reuseIdentifier)
        bounds = CGRect(x: 0, y: 0, width: Self.size, height: Self.size)
        host.view.backgroundColor = .clear
        host.view.frame = bounds
        host.view.isUserInteractionEnabled = false
        addSubview(host.view)
        // Below required, MapKit hides a pin that meets a handle's wide label frame.
        displayPriority = .required
        zPriority = .min
    }

    required init?(coder: NSCoder) { nil }

    func configure(kind: Stop.Kind, action: String?) {
        guard kind != self.kind || action != self.action else { return }
        self.kind = kind
        self.action = action
        host.rootView = AnyView(
            StopIcon(kind: kind, size: Self.size - 2, isRound: true)
                .overlay(Circle().strokeBorder(OBCTheme.panel, lineWidth: 1.5))
                .frame(width: Self.size, height: Self.size)
        )
        canShowCallout = true
        rightCalloutAccessoryView = action.map { title in
            var configuration = UIButton.Configuration.filled()
            configuration.title = title
            configuration.baseBackgroundColor = UIColor(OBCTheme.forest)
            configuration.baseForegroundColor = .white
            configuration.cornerStyle = .medium
            configuration.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 12, bottom: 8, trailing: 12)
            let button = UIButton(configuration: configuration)
            button.accessibilityIdentifier = "map.stopAction"
            // MapKit lays the accessory out by its frame: without one the button is 0 × 0.
            button.sizeToFit()
            return button
        }
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
