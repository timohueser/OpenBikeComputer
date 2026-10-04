#if os(iOS)
import MapKit
import MapLibre
import OBCPlanner
import OBCDomain
import SwiftUI

struct PlannerPreviewMapPin: Identifiable, Equatable {
    /// A `stop` sits on the route line, so it draws as a disc like the start; a `place` is a
    /// candidate off the line and hangs from a stem.
    /// `route` is the start of a signed route: numbered when it is listed, a dot otherwise.
    enum Kind: Equatable { case start, finish, stop, shape, marker, place, route(number: Int?, rank: Int) }
    let id: String
    let title: String
    let coordinate: Coordinate
    var symbol: String = "mappin"
    var kind: Kind = .place
    var highlighted = false
    var isAmbient = false
}

struct PlannerPreviewMap: UIViewRepresentable {
    let coordinates: [Coordinate]
    let pins: [PlannerPreviewMapPin]
    let selectedID: String?
    let cursor: Coordinate?
    let bottomInset: CGFloat
    let fitRevision: Int
    let onSelect: (String, CGPoint) -> Void
    let onMapPoint: (Coordinate, CGPoint) -> Void
    var showCycling = false
    var showHiking = false
    var onVisibleMapRect: (MKMapRect) -> Void = { _ in }
    var onVisibleRouteRange: (ClosedRange<Double>?) -> Void = { _ in }

    var release: PlannerRelease?
    var source: any PlannerDataSource = PlannerService.shared
    var hiddenCategories: Set<PlannerPreviewPlaceCategory> = []
    var highlightedCategories: Set<PlannerPreviewPlaceCategory> = []
    var onPlace: (PlannerPreviewPlace) -> Void = { _ in }
    var onNetworkStatus: (String?) -> Void = { _ in }
    /// Lines in place of the route line, such as the signed routes over the muted plan.
    var strokes: [MapStroke]?
    /// What a fit shows in place of the route and its pins.
    var focus: [Coordinate]?
    var namer: PlannerPlaceNamer?
    /// The map has settled and drawn its tiles.
    var onIdle: () -> Void = {}
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.obcIsOnline) private var online
    @Environment(\.obcOfflineMaps) private var offlineMaps

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> OBCNativeMapView {
        let map = OBCNativeMapView()
        namer?.map = map
        map.delegate = context.coordinator
        map.accessibilityIdentifier = "planner.map"
        map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
            guard let map, let coordinator else { return }
            coordinator.fit(map, animated: false)
        }
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapMap(_:)))
        tap.delegate = context.coordinator
        tap.cancelsTouchesInView = false
        map.addGestureRecognizer(tap)
        return map
    }

    func updateUIView(_ map: OBCNativeMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        map.load(release: release, dark: colorScheme == .dark, online: online, source: source, revision: offlineMaps?.revision ?? 0)
        map.attributionButtonMargins = CGPoint(x: 8, y: bottomInset + 8)
        coordinator.updatePOIs(map)
        if coordinator.coordinates != coordinates || coordinator.strokes != strokes {
            if coordinator.coordinates != coordinates {
                coordinator.coordinates = coordinates
                coordinator.routeIndex = SegmentedLineOverlay(line: MeasuredLine(routePoints: coordinates.map { RoutePoint(coordinate: $0) }))
            }
            coordinator.strokes = strokes
            coordinator.drawRoute(map)
        }
        coordinator.updateNetworks(map)
        let pinsChanged = coordinator.pins != pins
        if pinsChanged {
            coordinator.pins = pins
            map.removeAnnotations((map.annotations ?? []).filter { $0 !== coordinator.cursorAnnotation })
            map.addAnnotations(pins.map { PinAnnotation($0) })
        }
        if let cursor {
            coordinator.cursorAnnotation.coordinate = MapGeometry.clLocation(cursor)
            if !(map.annotations ?? []).contains(where: { $0 === coordinator.cursorAnnotation }) {
                map.addAnnotation(coordinator.cursorAnnotation)
            }
        } else {
            map.removeAnnotation(coordinator.cursorAnnotation)
        }
        if pinsChanged || coordinator.selectedID != selectedID || coordinator.pinAppearance != map.traitCollection.userInterfaceStyle {
            coordinator.selectedID = selectedID
            coordinator.pinAppearance = map.traitCollection.userInterfaceStyle
            for annotation in map.annotations ?? [] {
                guard let pin = annotation as? PinAnnotation, let view = map.view(for: pin) as? PlannerPinView else { continue }
                coordinator.style(view, pin: pin, traits: map.traitCollection)
            }
            coordinator.updatePlaceVisibility(map, force: true)
        }
        if coordinator.fitRevision != fitRevision {
            let animated = coordinator.fitRevision != nil
            coordinator.fitRevision = fitRevision
            // The first update precedes UIKit layout. Fit once the map has its screen bounds.
            DispatchQueue.main.async { coordinator.fit(map, animated: animated) }
        }
        coordinator.queueVisibleRange(map)
    }

    static func dismantleUIView(_ map: OBCNativeMapView, coordinator: Coordinator) {
        coordinator.networks.stop(); coordinator.places.stop(); map.stop()
    }

    final class PinAnnotation: MLNPointAnnotation {
        let pin: PlannerPreviewMapPin
        init(_ pin: PlannerPreviewMapPin) {
            self.pin = pin
            super.init()
            title = pin.title
            coordinate = MapGeometry.clLocation(pin.coordinate)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    }

    final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate, UIGestureRecognizerDelegate {
        var parent: PlannerPreviewMap
        var coordinates: [Coordinate] = []
        var strokes: [MapStroke]?
        var routeIndex: SegmentedLineOverlay?
        var pins: [PlannerPreviewMapPin] = []
        var fitRevision: Int?
        /// A tap on a pin reaches both native annotation selection and the map tap; the tap yields.
        private var pinSelectedAt: Date?
        var selectedID: String?
        var pinAppearance: UIUserInterfaceStyle?
        private var poiSelection: Set<String>?
        private var poiHighlights: Set<String>?
        private var ambientPlacesVisible: Bool?
        let cursorAnnotation = MLNPointAnnotation()
        private var visibleRangePending = false
        private var reportedRange: ClosedRange<Double>?
        private var hasReportedRange = false
        private var reportedViewport: MKMapRect?
        let networks = NativeViewportLayer(identifier: "networks")
        let places = NativeViewportLayer(identifier: "highlighted-places")
        private var layerStatuses: [String: String] = [:]
        private var reportedLayerStatus: String?

        private func reportLayerStatus(_ status: String?, layer: String) {
            layerStatuses[layer] = status
            let message = layerStatuses.keys.sorted().compactMap { layerStatuses[$0] }.first
            guard message != reportedLayerStatus else { return }
            reportedLayerStatus = message
            parent.onNetworkStatus(message)
        }

        init(_ parent: PlannerPreviewMap) {
            self.parent = parent
            cursorAnnotation.title = "Elevation position"
        }

        func fit(_ map: OBCNativeMapView, animated: Bool) {
            let points = parent.focus ?? (parent.coordinates + parent.pins.filter { !$0.isAmbient || $0.highlighted }.map(\.coordinate))
            map.fit(points, bottom: parent.bottomInset, animated: animated && !UIAccessibility.isReduceMotionEnabled)
        }

        func drawRoute(_ map: OBCNativeMapView, force: Bool = false) {
            map.draw(strokes ?? [MapStroke(coordinates: coordinates, color: OBCTheme.route, width: 3.5)], force: force)
        }
        func mapViewDidFinishLoadingMap(_ mapView: MLNMapView) {
            (mapView as? OBCNativeMapView)?.didFinishLoadingMap()
        }
        func mapViewDidBecomeIdle(_ mapView: MLNMapView) { parent.onIdle() }
        func mapViewDidFailLoadingMap(_ mapView: MLNMapView, withError error: Error) {
            (mapView as? OBCNativeMapView)?.didFailLoadingMap()
        }
        func mapView(_ map: MLNMapView, didFinishLoading style: MLNStyle) {
            guard let map = map as? OBCNativeMapView else { return }
            drawRoute(map, force: true)
            poiSelection = nil; poiHighlights = nil
            updatePOIs(map)
            updateNetworks(map)
        }
        func updateNetworks(_ map: OBCNativeMapView) {
            networks.status = { [weak self] in self?.reportLayerStatus($0, layer: "networks") }
            let network = parent.showCycling ? "cycling" : parent.showHiking ? "hiking" : "none"
            if map.style?.source(withIdentifier: "networks") is MLNVectorTileSource {
                networks.update(map, key: nil, release: nil) { _, _, _ in Data() }
                for id in ["planner-networks", "planner-network-labels"] {
                    for kind in ["cycling", "hiking"] { map.style?.layer(withIdentifier: "\(id)-\(kind)")?.isVisible = kind == network }
                }
                return
            }
            let source = parent.source
            networks.update(map, key: network == "none" ? nil : network, release: map.selectedRelease, minimumZoom: 6) { bounds, zoom, release in
                try await source.overlays(bounds: bounds, zoom: zoom, network: network, release: release)
            }
        }
        func updatePOIs(_ map: OBCNativeMapView) {
            let shown = Set(PlannerPreviewPlaceCategory.allCases.filter { !parent.hiddenCategories.contains($0) }.map(\.rawValue))
            let highlighted = Set(parent.highlightedCategories.map(\.rawValue))
            let visibilityChanged = shown != poiSelection
            if visibilityChanged {
                poiSelection = shown
                let predicate = NSPredicate(format: "kind IN %@", NativePlaceKind.kinds(in: shown))
                (map.style?.layer(withIdentifier: "planner-pois") as? MLNCircleStyleLayer)?.predicate = predicate
                (map.style?.layer(withIdentifier: "planner-poi-icons") as? MLNSymbolStyleLayer)?.predicate = predicate
            }
            let source = parent.source
            let categories = highlighted.intersection(shown)
            places.status = { [weak self] in self?.reportLayerStatus($0, layer: "places") }
            places.update(map, key: categories.isEmpty ? nil : categories.sorted().joined(separator: ","),
                          release: map.selectedRelease, maximumZoom: 13) { bounds, _, release in
                var query = PlannerSearchQuery(text: "places", view: bounds)
                query.kinds = NativePlaceKind.searchKinds(in: categories)
                return try await NativePlaceKind.geoJSON(source.search(query, release: release))
            }
            if highlighted != poiHighlights || visibilityChanged {
                poiHighlights = highlighted
                (map.style?.layer(withIdentifier: "planner-poi-highlight") as? MLNCircleStyleLayer)?.predicate =
                    NSPredicate(format: "kind IN %@", NativePlaceKind.kinds(in: highlighted.intersection(shown)))
            }
        }

        func updatePlaceVisibility(_ map: MLNMapView, force: Bool = false) {
            let visible = showsAmbientPlaces(map)
            guard force || ambientPlacesVisible != visible else { return }
            ambientPlacesVisible = visible
            for annotation in map.annotations ?? [] {
                guard let pin = annotation as? PinAnnotation, let view = map.view(for: pin) as? PlannerPinView else { continue }
                setVisibility(view, pin: pin, ambientVisible: visible)
            }
        }

        private func showsAmbientPlaces(_ map: MLNMapView) -> Bool {
            // Match planner-poi-icons in builder/app/src/lib/planner/map-style.ts.
            map.zoomLevel >= 13
        }

        private func setVisibility(_ view: PlannerPinView, pin: PinAnnotation, ambientVisible: Bool) {
            let important = !pin.pin.isAmbient || pin.pin.highlighted || pin.pin.id == parent.selectedID
            let visible = important || ambientVisible
            view.isHidden = !visible
            view.isUserInteractionEnabled = visible
            view.isAccessibilityElement = visible
            view.accessibilityElementsHidden = !visible
        }

        func mapView(_ map: MLNMapView, viewFor annotation: any MLNAnnotation) -> MLNAnnotationView? {
            if annotation === cursorAnnotation {
                let view = PlannerPinView(reuseIdentifier: "cursor")
                view.image = UIGraphicsImageRenderer(size: CGSize(width: 16, height: 16)).image { _ in
                    UIColor(OBCTheme.surface).setFill()
                    UIBezierPath(ovalIn: CGRect(x: 1, y: 1, width: 14, height: 14)).fill()
                    UIColor(OBCTheme.amber).setFill()
                    UIBezierPath(ovalIn: CGRect(x: 4, y: 4, width: 8, height: 8)).fill()
                }
                view.isUserInteractionEnabled = false
                    return view
            }
            guard let pin = annotation as? PinAnnotation else { return nil }
            let view = map.dequeueReusableAnnotationView(withIdentifier: "pin") as? PlannerPinView
                ?? PlannerPinView(reuseIdentifier: "pin")
            view.annotation = annotation
            view.accessibilityLabel = pin.pin.title
            view.accessibilityIdentifier = "planner.pin.\(pin.pin.id)"
            style(view, pin: pin, traits: map.traitCollection)
            setVisibility(view, pin: pin, ambientVisible: showsAmbientPlaces(map))
            return view
        }

        func style(_ view: PlannerPinView, pin: PinAnnotation, traits: UITraitCollection) {
            let selected = pin.pin.id == parent.selectedID
            let kind = pin.pin.kind
            if case .route(let number, let rank) = kind {
                view.centerOffset = .zero
                view.image = Self.routeImage(number: number, rank: rank, selected: selected, traits: traits)
                return
            }
            let stemmed = kind == .marker || (kind == .place && !pin.pin.isAmbient)
            view.centerOffset = CGVector(dx: 0, dy: stemmed ? -12 : 0)
            view.image = UIGraphicsImageRenderer(size: CGSize(width: 44, height: 44)).image { _ in
                let outline = UIColor(OBCTheme.secondary).resolvedColor(with: traits)
                let surface = UIColor(selected ? OBCTheme.amber : OBCTheme.surface).resolvedColor(with: traits)
                let shape: UIBezierPath
                if kind == .shape {
                    shape = UIBezierPath()
                    shape.move(to: CGPoint(x: 22, y: 15)); shape.addLine(to: CGPoint(x: 29, y: 22))
                    shape.addLine(to: CGPoint(x: 22, y: 29)); shape.addLine(to: CGPoint(x: 15, y: 22))
                    shape.close()
                } else if stemmed {
                    shape = UIBezierPath()
                    shape.move(to: CGPoint(x: 22, y: 35))
                    shape.addCurve(to: CGPoint(x: 9, y: 17), controlPoint1: CGPoint(x: 17, y: 29), controlPoint2: CGPoint(x: 9, y: 25))
                    shape.addArc(withCenter: CGPoint(x: 22, y: 17), radius: 13, startAngle: .pi, endAngle: 0, clockwise: true)
                    shape.addCurve(to: CGPoint(x: 22, y: 35), controlPoint1: CGPoint(x: 35, y: 25), controlPoint2: CGPoint(x: 27, y: 29))
                    shape.close()
                } else {
                    shape = kind == .finish
                        ? UIBezierPath(roundedRect: CGRect(x: 8, y: 8, width: 28, height: 28), cornerRadius: 7)
                        : UIBezierPath(ovalIn: CGRect(x: 8, y: 8, width: 28, height: 28))
                }
                if pin.pin.highlighted {
                    UIColor(OBCTheme.amber).resolvedColor(with: traits).setStroke()
                    let ring = UIBezierPath(ovalIn: CGRect(x: 3, y: stemmed ? 0 : 3, width: 38, height: 38))
                    ring.lineWidth = 3; ring.stroke()
                }
                surface.setFill(); shape.fill()
                outline.setStroke(); shape.lineWidth = selected ? 2 : 1.5; shape.stroke()
                guard kind != .shape else { return }
                let symbol = kind == .start ? "play.fill" : kind == .finish ? "flag.checkered" : pin.pin.symbol
                let config = UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold)
                let image = UIImage(systemName: symbol, withConfiguration: config)?.withTintColor(outline, renderingMode: .alwaysOriginal)
                let center = CGPoint(x: 22, y: stemmed ? 17 : 22)
                if let image {
                    image.draw(at: CGPoint(x: center.x - image.size.width / 2, y: center.y - image.size.height / 2))
                }
            }
        }

        /// A numbered disc in the network colour, or a small dot; magenta when selected.
        static func routeImage(number: Int?, rank: Int, selected: Bool, traits: UITraitCollection) -> UIImage {
            let color = selected ? UIColor(OBCTheme.route).resolvedColor(with: traits)
                : PlannerPreviewNetworkStyle.color(rank: rank, traits: traits).withAlphaComponent(1)
            let surface = UIColor(OBCTheme.surface).resolvedColor(with: traits)
            let size: CGFloat = number == nil ? 14 : 28
            return UIGraphicsImageRenderer(size: CGSize(width: size, height: size)).image { _ in
                let disc = UIBezierPath(ovalIn: CGRect(x: 1, y: 1, width: size - 2, height: size - 2))
                (number == nil || selected ? color : surface).setFill(); disc.fill()
                (number == nil ? surface : color).setStroke(); disc.lineWidth = number == nil ? 1.5 : 2; disc.stroke()
                guard let number else { return }
                let text = NSAttributedString(string: "\(number)", attributes: [
                    .font: UIFont.monospacedDigitSystemFont(ofSize: 11, weight: .semibold),
                    .foregroundColor: selected ? surface : UIColor(OBCTheme.ink).resolvedColor(with: traits)])
                let bounds = text.size()
                text.draw(at: CGPoint(x: (size - bounds.width) / 2, y: (size - bounds.height) / 2))
            }
        }

        func mapViewRegionIsChanging(_ map: MLNMapView) {
            updatePlaceVisibility(map)
            queueVisibleRange(map)
        }
        func mapView(_ map: MLNMapView, regionDidChangeAnimated animated: Bool) {
            if let map = map as? OBCNativeMapView { updateNetworks(map); updatePOIs(map); map.updateCoverageStatus(); map.viewportSettled() }
            updatePlaceVisibility(map)
            queueVisibleRange(map)
        }

        func queueVisibleRange(_ map: MLNMapView) {
            guard !visibleRangePending else { return }
            visibleRangePending = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [weak self, weak map] in
                guard let self, let map else { return }
                self.visibleRangePending = false
                let visible = map.bounds.inset(by: UIEdgeInsets(top: map.safeAreaInsets.top, left: 0,
                    bottom: max(self.parent.bottomInset, map.safeAreaInsets.bottom), right: 0))
                let northWest = MKMapPoint(map.convert(CGPoint(x: visible.minX, y: visible.minY), toCoordinateFrom: map))
                let southEast = MKMapPoint(map.convert(CGPoint(x: visible.maxX, y: visible.maxY), toCoordinateFrom: map))
                let viewport = MKMapRect(x: min(northWest.x, southEast.x), y: min(northWest.y, southEast.y),
                    width: abs(southEast.x - northWest.x), height: abs(southEast.y - northWest.y))
                let unchanged = self.reportedViewport.map {
                    abs($0.origin.x - viewport.origin.x) < 0.1 && abs($0.origin.y - viewport.origin.y) < 0.1
                        && abs($0.size.width - viewport.size.width) < 0.1 && abs($0.size.height - viewport.size.height) < 0.1
                } ?? false
                if !unchanged {
                    self.reportedViewport = viewport
                    self.parent.onVisibleMapRect(viewport)
                }
                let range = self.visibleRange(map)
                guard !self.hasReportedRange || range != self.reportedRange else { return }
                self.hasReportedRange = true
                self.reportedRange = range
                self.parent.onVisibleRouteRange(range)
            }
        }

        private func visibleRange(_ map: MLNMapView) -> ClosedRange<Double>? {
            guard let index = routeIndex, index.line.length > 0 else { return nil }
            let rect = map.bounds.inset(by: UIEdgeInsets(top: map.safeAreaInsets.top, left: map.safeAreaInsets.left,
                bottom: max(parent.bottomInset, map.safeAreaInsets.bottom), right: map.safeAreaInsets.right))
            guard rect.width > 0, rect.height > 0 else { return nil }
            let nw = MKMapPoint(map.convert(CGPoint(x: rect.minX, y: rect.minY), toCoordinateFrom: map))
            let se = MKMapPoint(map.convert(CGPoint(x: rect.maxX, y: rect.maxY), toCoordinateFrom: map))
            let viewport = MKMapRect(x: min(nw.x, se.x), y: min(nw.y, se.y), width: abs(se.x - nw.x), height: abs(se.y - nw.y))
            let pieces = index.pieces(in: viewport).pieces
            guard let first = pieces.first, let last = pieces.last else { return nil }
            return max(0, first.lowerBound / index.line.length)...min(1, last.upperBound / index.line.length)
        }

        func mapView(_ map: MLNMapView, didSelect annotation: any MLNAnnotation) {
            guard let pin = annotation as? PinAnnotation else { return }
            pinSelectedAt = Date()
            parent.onSelect(pin.pin.id, map.convert(pin.coordinate, toPointTo: map))
            map.deselectAnnotation(annotation, animated: false)
        }

        @objc func tapMap(_ recognizer: UITapGestureRecognizer) {
            guard let map = recognizer.view as? OBCNativeMapView else { return }
            let location = recognizer.location(in: map)
            let point = map.convert(location, toCoordinateFrom: map)
            // Native annotation's selection may land just after this tap; give it a moment, then yield to it.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
                guard let self, self.pinSelectedAt.map({ Date().timeIntervalSince($0) > 0.4 }) ?? true else { return }
                let features = map.visibleFeatures(in: CGRect(x: location.x - 10, y: location.y - 10, width: 20, height: 20),
                                                   styleLayerIdentifiers: ["planner-pois", "planner-poi-icons", "highlighted-place-icons"])
                if let feature = features.first as? MLNPointFeature,
                   let kind = feature.attributes["kind"] as? String {
                    let name = feature.attributes["name:en"] as? String ?? feature.attributes["name"] as? String
                        ?? NativePlaceKind.entries[kind]?.label ?? "Place"
                    let coordinate = Coordinate(latitude: feature.coordinate.latitude, longitude: feature.coordinate.longitude)
                    let id = feature.identifier.map { String(describing: $0) } ?? "\(kind)-\(coordinate.latitude)-\(coordinate.longitude)"
                    self.parent.onPlace(.init(id: id, name: name, coordinate: coordinate, kind: NativePlaceKind.kind(for: kind),
                                              hours: feature.attributes["opening_hours"] as? String))
                    return
                }
                self.parent.onMapPoint(Coordinate(latitude: point.latitude, longitude: point.longitude), location)
            }
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            var view = touch.view
            while let current = view {
                if current is MLNAnnotationView { return false }
                view = current.superview
            }
            return true
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer,
                               shouldRequireFailureOf otherGestureRecognizer: UIGestureRecognizer) -> Bool {
            guard let tap = otherGestureRecognizer as? UITapGestureRecognizer else { return false }
            return tap.numberOfTapsRequired > 1
        }
    }
}

/// Names a point by the basemap place nearest to it, from the loaded tiles.
@MainActor final class PlannerPlaceNamer {
    weak var map: MLNMapView?

    /// The nearest village, town or city within 5 km, from the loaded tiles and the shown labels.
    func name(near coordinate: Coordinate) -> String? {
        guard let map else { return nil }
        let loaded = (map.style?.source(withIdentifier: "basemap") as? MLNVectorTileSource)?
            .features(sourceLayerIdentifiers: ["places"], predicate: nil) ?? []
        let kx = cos(coordinate.latitude * .pi / 180)
        var best: (name: String, km: Double)?
        for case let feature as MLNPointFeature in loaded + map.visibleFeatures(in: map.bounds, styleLayerIdentifiers: ["places_locality"]) {
            guard feature.attributes["kind"] as? String == "locality", let name = feature.attributes["name"] as? String else { continue }
            let dx = (feature.coordinate.longitude - coordinate.longitude) * kx, dy = feature.coordinate.latitude - coordinate.latitude
            let km = (dx * dx + dy * dy).squareRoot() * 111.2
            if km < 5, km < best?.km ?? .infinity { best = (name, km) }
        }
        return best?.name
    }
}

final class PlannerPinView: MLNAnnotationView {
    private let drawing = UIImageView()
    var image: UIImage? {
        didSet { drawing.image = image; frame.size = image?.size ?? .zero; drawing.frame = bounds }
    }
    override init(reuseIdentifier: String?) { super.init(reuseIdentifier: reuseIdentifier); addSubview(drawing) }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
}
#endif
