#if DEBUG && os(iOS)
import MapKit
import OBCDomain
import SwiftUI

struct PlannerPreviewMapPin: Identifiable, Equatable {
    /// A `stop` sits on the route line, so it draws as a disc like the start; a `place` is a
    /// candidate off the line and hangs from a stem.
    enum Kind: Equatable { case start, finish, stop, shape, marker, place }
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
    var onSelectionPosition: (CGPoint) -> Void = { _ in }
    var onVisibleRouteRange: (ClosedRange<Double>?) -> Void = { _ in }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> MKMapView {
        let map = MKMapView()
        map.delegate = context.coordinator
        map.pointOfInterestFilter = .excludingAll
        map.showsCompass = false
        map.isPitchEnabled = false
        map.accessibilityIdentifier = "planner.map"
        map.setRegion(MapGeometry.boundingRegion(for: [
            coordinates.first ?? pins.first?.coordinate
                ?? Coordinate(latitude: 48.137, longitude: 11.575)
        ], pad: 1.3), animated: false)
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapMap(_:)))
        tap.delegate = context.coordinator
        tap.cancelsTouchesInView = false
        map.addGestureRecognizer(tap)
        return map
    }

    func updateUIView(_ map: MKMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        map.layoutMargins.bottom = bottomInset + 8
        if coordinator.coordinates != coordinates {
            coordinator.coordinates = coordinates
            coordinator.distances = coordinates.reduce(into: [Double]()) { distances, coordinate in
                let index = distances.count
                distances.append(index == 0 ? 0 : distances[index - 1] + coordinates[index - 1].routeDistance(to: coordinate))
            }
            map.removeOverlays(map.overlays.filter { $0 is MKPolyline })
            if coordinates.count > 1 {
                let points = MapGeometry.clLocations(coordinates)
                let casing = MKPolyline(coordinates: points, count: points.count)
                casing.title = "casing"
                map.addOverlays([casing, MKPolyline(coordinates: points, count: points.count)])
            }
        }
        coordinator.updateNetworks(map)
        let pinsChanged = coordinator.pins != pins
        if pinsChanged {
            coordinator.pins = pins
            map.removeAnnotations(map.annotations.filter { $0 !== coordinator.cursorAnnotation && !($0 is PlannerPreviewNetworkLabel) })
            map.addAnnotations(pins.map { PinAnnotation($0) })
        }
        if let cursor {
            coordinator.cursorAnnotation.coordinate = MapGeometry.clLocation(cursor)
            if !map.annotations.contains(where: { $0 === coordinator.cursorAnnotation }) {
                map.addAnnotation(coordinator.cursorAnnotation)
            }
        } else {
            map.removeAnnotation(coordinator.cursorAnnotation)
        }
        if pinsChanged || coordinator.selectedID != selectedID || coordinator.pinAppearance != map.traitCollection.userInterfaceStyle {
            coordinator.selectedID = selectedID
            coordinator.pinAppearance = map.traitCollection.userInterfaceStyle
            for annotation in map.annotations {
                guard let pin = annotation as? PinAnnotation, let view = map.view(for: pin) else { continue }
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

    final class PinAnnotation: MKPointAnnotation {
        let pin: PlannerPreviewMapPin
        init(_ pin: PlannerPreviewMapPin) {
            self.pin = pin
            super.init()
            title = pin.title
            coordinate = MapGeometry.clLocation(pin.coordinate)
        }
    }

    final class Coordinator: NSObject, MKMapViewDelegate, UIGestureRecognizerDelegate {
        var parent: PlannerPreviewMap
        var coordinates: [Coordinate] = []
        var distances: [Double] = []
        var pins: [PlannerPreviewMapPin] = []
        var fitRevision: Int?
        var selectedID: String?
        var pinAppearance: UIUserInterfaceStyle?
        private var networkAppearance: UIUserInterfaceStyle?
        private var networkLabelsVisible: Bool?
        private var ambientPlacesVisible: Bool?
        let cursorAnnotation = MKPointAnnotation()
        private var visibleRangePending = false
        private var reportedRange: ClosedRange<Double>?
        private var hasReportedRange = false
        private var reportedViewport: MKMapRect?
        private var reportedSelection: (id: String, point: CGPoint)?
        private var networkOverlays: [String: [PlannerPreviewNetworkOverlay]] = [:]

        init(_ parent: PlannerPreviewMap) {
            self.parent = parent
            cursorAnnotation.title = "Elevation position"
        }

        func fit(_ map: MKMapView, animated: Bool) {
            // The route, its points and any search hits; ambient places would drag the fit out to
            // the whole sample area.
            let points = parent.coordinates + parent.pins.filter { !$0.isAmbient || $0.highlighted }.map(\.coordinate)
            guard !points.isEmpty else { return }
            let bounds = points.reduce(MKMapRect.null) { rect, coordinate in
                let point = MKMapPoint(MapGeometry.clLocation(coordinate))
                return rect.union(MKMapRect(x: point.x, y: point.y, width: 1, height: 1))
            }
            // The layout margin already reserves the drawer's height.
            let padding = UIEdgeInsets(top: 60, left: 48, bottom: 35, right: 64)
            map.setVisibleMapRect(bounds.insetBy(dx: -400, dy: -400), edgePadding: padding,
                animated: animated && !UIAccessibility.isReduceMotionEnabled)
        }

        func mapView(_ map: MKMapView, rendererFor overlay: any MKOverlay) -> MKOverlayRenderer {
            if let network = overlay as? PlannerPreviewNetworkOverlay {
                let renderer = PlannerPreviewNetworkRenderer(multiPolyline: network)
                renderer.strokeColor = PlannerPreviewNetworkStyle.color(rank: network.rank, traits: map.traitCollection)
                renderer.lineCap = .butt
                renderer.lineJoin = .round
                return renderer
            }
            guard let line = overlay as? MKPolyline else { return MKOverlayRenderer(overlay: overlay) }
            let renderer = MKPolylineRenderer(polyline: line)
            let casing = line.title == "casing"
            renderer.strokeColor = UIColor(casing ? OBCTheme.routeCasing : OBCTheme.route)
            renderer.lineWidth = casing ? 7 : 3.5
            renderer.lineCap = .round
            renderer.lineJoin = .round
            return renderer
        }

        func updateNetworks(_ map: MKMapView) {
            for (network, shown) in [("cycling", parent.showCycling), ("hiking", parent.showHiking)] {
                if shown && networkOverlays[network] == nil {
                    let overlays = PlannerPreviewNetworkData.overlays(network: network)
                    networkOverlays[network] = overlays
                    map.addOverlays(overlays, level: .aboveRoads)
                    map.addAnnotations(PlannerPreviewNetworkData.labels(network: network))
                } else if !shown, let overlays = networkOverlays.removeValue(forKey: network) {
                    map.removeOverlays(overlays)
                    map.removeAnnotations(map.annotations.compactMap { $0 as? PlannerPreviewNetworkLabel }.filter { $0.network == network })
                }
            }
            if networkAppearance != map.traitCollection.userInterfaceStyle {
                networkAppearance = map.traitCollection.userInterfaceStyle
                for overlay in networkOverlays.values.flatMap({ $0 }) {
                    guard let renderer = map.renderer(for: overlay) as? PlannerPreviewNetworkRenderer else { continue }
                    renderer.strokeColor = PlannerPreviewNetworkStyle.color(rank: overlay.rank, traits: map.traitCollection)
                }
                for annotation in map.annotations {
                    guard let label = annotation as? PlannerPreviewNetworkLabel, let view = map.view(for: label) else { continue }
                    view.image = label.image(traits: map.traitCollection)
                }
            }
            updateNetworkLabels(map)
        }

        private func updateNetworkLabels(_ map: MKMapView) {
            let visible = PlannerPreviewNetworkStyle.zoom(map.bounds.width / map.visibleMapRect.width) >= 11
            guard networkLabelsVisible != visible else { return }
            networkLabelsVisible = visible
            for annotation in map.annotations where annotation is PlannerPreviewNetworkLabel {
                map.view(for: annotation)?.isHidden = !visible
            }
        }

        func updatePlaceVisibility(_ map: MKMapView, force: Bool = false) {
            let visible = showsAmbientPlaces(map)
            guard force || ambientPlacesVisible != visible else { return }
            ambientPlacesVisible = visible
            for annotation in map.annotations {
                guard let pin = annotation as? PinAnnotation, let view = map.view(for: pin) else { continue }
                setVisibility(view, pin: pin, ambientVisible: visible)
            }
        }

        private func showsAmbientPlaces(_ map: MKMapView) -> Bool {
            // Match planner-poi-icons in builder/app/src/lib/planner/map-style.ts.
            PlannerPreviewNetworkStyle.zoom(map.bounds.width / map.visibleMapRect.width) >= 13
        }

        private func setVisibility(_ view: MKAnnotationView, pin: PinAnnotation, ambientVisible: Bool) {
            let important = !pin.pin.isAmbient || pin.pin.highlighted || pin.pin.id == parent.selectedID
            let visible = important || ambientVisible
            view.isHidden = !visible
            view.isEnabled = visible
            view.isAccessibilityElement = visible
            view.accessibilityElementsHidden = !visible
            view.displayPriority = important ? .required : .defaultLow
        }

        func mapView(_ map: MKMapView, viewFor annotation: any MKAnnotation) -> MKAnnotationView? {
            if let label = annotation as? PlannerPreviewNetworkLabel {
                let view = map.dequeueReusableAnnotationView(withIdentifier: "network-label")
                    ?? MKAnnotationView(annotation: annotation, reuseIdentifier: "network-label")
                view.annotation = annotation
                view.image = label.image(traits: map.traitCollection)
                view.centerOffset = CGPoint(x: 0, y: 9)
                view.collisionMode = .rectangle
                view.displayPriority = .defaultLow
                view.isEnabled = false
                view.isAccessibilityElement = false
                view.isHidden = PlannerPreviewNetworkStyle.zoom(map.bounds.width / map.visibleMapRect.width) < 11
                return view
            }
            if annotation === cursorAnnotation {
                let view = MKAnnotationView(annotation: annotation, reuseIdentifier: "cursor")
                view.image = UIGraphicsImageRenderer(size: CGSize(width: 16, height: 16)).image { _ in
                    UIColor(OBCTheme.surface).setFill()
                    UIBezierPath(ovalIn: CGRect(x: 1, y: 1, width: 14, height: 14)).fill()
                    UIColor(OBCTheme.amber).setFill()
                    UIBezierPath(ovalIn: CGRect(x: 4, y: 4, width: 8, height: 8)).fill()
                }
                view.isEnabled = false
                view.displayPriority = .required
                return view
            }
            guard let pin = annotation as? PinAnnotation else { return nil }
            let view = map.dequeueReusableAnnotationView(withIdentifier: "pin")
                ?? MKAnnotationView(annotation: annotation, reuseIdentifier: "pin")
            view.annotation = annotation
            view.displayPriority = .required
            view.collisionMode = .circle
            view.accessibilityLabel = pin.pin.title
            view.accessibilityIdentifier = "planner.pin.\(pin.pin.id)"
            style(view, pin: pin, traits: map.traitCollection)
            setVisibility(view, pin: pin, ambientVisible: showsAmbientPlaces(map))
            return view
        }

        func style(_ view: MKAnnotationView, pin: PinAnnotation, traits: UITraitCollection) {
            let selected = pin.pin.id == parent.selectedID
            let kind = pin.pin.kind
            let stemmed = kind == .marker || (kind == .place && !pin.pin.isAmbient)
            view.centerOffset = CGPoint(x: 0, y: stemmed ? -12 : 0)
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

        func mapViewDidChangeVisibleRegion(_ map: MKMapView) {
            updateNetworkLabels(map)
            updatePlaceVisibility(map)
            queueVisibleRange(map)
        }

        func queueVisibleRange(_ map: MKMapView) {
            guard !visibleRangePending else { return }
            visibleRangePending = true
            DispatchQueue.main.async { [weak self, weak map] in
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
                if let selectedID = self.parent.selectedID,
                   let pin = self.parent.pins.first(where: { $0.id == selectedID }) {
                    let point = map.convert(MapGeometry.clLocation(pin.coordinate), toPointTo: map)
                    let moved = self.reportedSelection.map {
                        let dx = point.x - $0.point.x, dy = point.y - $0.point.y
                        return dx * dx + dy * dy > 0.25
                    } ?? true
                    if self.reportedSelection?.id != selectedID || moved {
                        self.reportedSelection = (selectedID, point)
                        self.parent.onSelectionPosition(point)
                    }
                } else {
                    self.reportedSelection = nil
                }
                let range = self.visibleRange(map)
                guard !self.hasReportedRange || range != self.reportedRange else { return }
                self.hasReportedRange = true
                self.reportedRange = range
                self.parent.onVisibleRouteRange(range)
            }
        }

        private func visibleRange(_ map: MKMapView) -> ClosedRange<Double>? {
            guard coordinates.count > 1, let total = distances.last, total > 0 else { return nil }
            let rect = map.bounds.inset(by: UIEdgeInsets(top: map.safeAreaInsets.top, left: map.safeAreaInsets.left,
                bottom: max(parent.bottomInset, map.safeAreaInsets.bottom), right: map.safeAreaInsets.right))
            guard rect.width > 0, rect.height > 0 else { return nil }
            let points = coordinates.map { map.convert(MapGeometry.clLocation($0), toPointTo: map) }
            var low = Double.infinity, high = -Double.infinity
            for index in 1..<points.count {
                guard let clipped = Self.clip(from: points[index - 1], to: points[index], in: rect) else { continue }
                let length = distances[index] - distances[index - 1]
                low = min(low, distances[index - 1] + length * clipped.lowerBound)
                high = max(high, distances[index - 1] + length * clipped.upperBound)
            }
            guard low.isFinite, high >= low else { return nil }
            return max(0, low / total)...min(1, high / total)
        }

        // Clip whole segments so a crossing is included even when both vertices are outside.
        private static func clip(from a: CGPoint, to b: CGPoint, in rect: CGRect) -> ClosedRange<Double>? {
            let dx = b.x - a.x, dy = b.y - a.y
            var low = 0.0, high = 1.0
            for (direction, offset) in [(-dx, a.x - rect.minX), (dx, rect.maxX - a.x),
                                         (-dy, a.y - rect.minY), (dy, rect.maxY - a.y)] {
                if abs(direction) < 0.000001 {
                    if offset < 0 { return nil }
                } else {
                    let fraction = Double(offset / direction)
                    if direction < 0 { low = max(low, fraction) } else { high = min(high, fraction) }
                    if low > high { return nil }
                }
            }
            return low...high
        }

        func mapView(_ map: MKMapView, didSelect annotation: any MKAnnotation) {
            guard let pin = annotation as? PinAnnotation else { return }
            parent.onSelect(pin.pin.id, map.convert(pin.coordinate, toPointTo: map))
            map.deselectAnnotation(annotation, animated: false)
        }

        @objc func tapMap(_ recognizer: UITapGestureRecognizer) {
            guard let map = recognizer.view as? MKMapView else { return }
            let point = map.convert(recognizer.location(in: map), toCoordinateFrom: map)
            parent.onMapPoint(Coordinate(latitude: point.latitude, longitude: point.longitude), recognizer.location(in: map))
        }

        func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
            var view = touch.view
            while let current = view {
                if current is MKAnnotationView { return false }
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
#endif
