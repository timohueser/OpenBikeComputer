#if os(iOS)
import MapLibre
import MapKit
import OBCDomain
import OBCPlanner
import SwiftUI

struct MapStroke: Equatable {
    var coordinates: [Coordinate]
    var color: Color
    var width: CGFloat = 3.4
    var cased = true
    var casingColor: Color = OBCTheme.routeCasing
    var dash: [CGFloat] = []
}

struct MapPin: Equatable {
    var coordinate: Coordinate
    var color: Color = OBCTheme.ink
    var label = ""
    var square = false
    var size: CGFloat = 10
    var symbol: String?
    var photo: Bool?
}

/// One native renderer for previews, planning, and editing. Sources change only at rest.
@MainActor
final class OBCNativeMapView: MLNMapView {
    var onFirstLayout: (() -> Void)?
    private var laidOut = false
    private var loading: Task<Void, Never>?
    private var retry: (() -> Void)?
    private var styleKey: String?
    private var selectedRelease: PlannerRelease?
    private let notice = UIButton(type: .system)
    private var availability = NativeMapLoadState.loading
    private var strokes: [MapStroke]?

    init() {
        super.init(frame: .zero, styleURL: Bundle.module.url(forResource: "blank", withExtension: "json", subdirectory: "Map")!)
        isRotateEnabled = false
        isPitchEnabled = false
        logoView.isHidden = true
        notice.titleLabel?.font = .preferredFont(forTextStyle: .caption1)
        notice.setTitleColor(UIColor(OBCTheme.ink), for: .normal)
        notice.backgroundColor = UIColor(OBCTheme.surface)
        notice.layer.cornerRadius = 5; notice.clipsToBounds = true
        notice.titleLabel?.numberOfLines = 2
        notice.accessibilityIdentifier = "map.availability"
        notice.isHidden = true
        addSubview(notice)
        notice.isUserInteractionEnabled = true
        notice.addTarget(self, action: #selector(retryLoading), for: .touchUpInside)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        let size = notice.sizeThatFits(CGSize(width: max(0, bounds.width - 90), height: 60))
        notice.frame = CGRect(x: 8, y: safeAreaInsets.top + 8, width: size.width + 12, height: size.height + 8)
        if !laidOut, bounds.width > 0, bounds.height > 0 {
            laidOut = true
            onFirstLayout?()
        }
    }

    func load(release: PlannerRelease? = nil, dark: Bool, online: Bool = true, source: any PlannerDataSource = PlannerService.shared) {
        let key = "\(release?.id ?? "active")-\(dark)-\(online)"
        guard key != styleKey else { return }
        styleKey = key
        retry = { [weak self] in
            self?.styleKey = nil
            self?.load(release: release, dark: dark, online: online, source: source)
        }
        loading?.cancel()
        if !online {
            availability = .offline
            updateCoverageStatus()
            styleURL = Bundle.module.url(forResource: "blank", withExtension: "json", subdirectory: "Map")!
            return
        }
        availability = .loading
        updateCoverageStatus()
        loading = Task { [weak self] in
            do {
                let selected: PlannerRelease
                if let release { selected = release } else { selected = try await source.release() }
                try Task.checkCancellation()
                let url = try Self.styleURL(release: selected, dark: dark)
                guard let self else { return }
                self.selectedRelease = selected
                if self.styleURL == url { self.reloadStyle(nil) } else { self.styleURL = url }
                if abs(self.centerCoordinate.latitude) < 0.01, abs(self.centerCoordinate.longitude) < 0.01 {
                    self.setCenter(CLLocationCoordinate2D(latitude: (selected.bounds[1] + selected.bounds[3]) / 2,
                        longitude: (selected.bounds[0] + selected.bounds[2]) / 2), zoomLevel: 8, animated: false)
                }
            } catch is CancellationError {} catch {
                // The drawn track stays visible when the basemap cannot load.
                guard !Task.isCancelled else { return }
                self?.availability = .failed
                self?.updateCoverageStatus()
            }
        }
    }

    @objc private func retryLoading() { retry?() }

    func showNotice(_ text: String?) {
        notice.setTitle(text, for: .normal); notice.isHidden = text == nil; setNeedsLayout()
    }
    func updateCoverageStatus() {
        let bounds = visibleCoordinateBounds
        let outside = selectedRelease.map { release in
            bounds.ne.longitude < release.bounds[0] || bounds.sw.longitude > release.bounds[2]
                || bounds.ne.latitude < release.bounds[1] || bounds.sw.latitude > release.bounds[3]
        } ?? false
        showNotice(availability.message(outsideRegion: outside))
    }

    func didFinishLoadingMap() {
        guard availability == .loading, styleURL.lastPathComponent != "blank.json" else { return }
        availability = .ready
        updateCoverageStatus()
    }
    func didFailLoadingMap() {
        guard availability != .offline else { return }
        availability = .failed
        updateCoverageStatus()
    }

    func stop() { loading?.cancel(); loading = nil; delegate = nil }

    private static func styleURL(release: PlannerRelease, dark: Bool) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appending(path: "OBCMapStyles")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appending(path: "\(release.id)-\(dark ? "dark" : "light").json")
        let template = Bundle.module.url(forResource: dark ? "dark" : "light", withExtension: "json", subdirectory: "Map")!
        var data = try JSONSerialization.jsonObject(with: Data(contentsOf: template)) as! [String: Any]
        data["glyphs"] = release.glyphs
        data["sprite"] = release.sprites + (dark ? "/dark" : "/light")
        var sources = data["sources"] as! [String: [String: Any]]
        sources["basemap"]?["url"] = release.basemap.absoluteString
        sources["terrain"]?["tiles"] = [release.terrain]
        sources["terrain"]?["bounds"] = release.bounds
        sources["terrain"]?["attribution"] = release.terrain_attribution
        data["sources"] = sources
        try JSONSerialization.data(withJSONObject: data).write(to: url, options: .atomic)
        return url
    }

    private func installPOIImages(_ style: MLNStyle) {
        // The shared web style creates these images at runtime; they are absent from its sprite atlas.
        guard style.image(forName: "poi-camp-light") == nil else { return }
        for dark in [false, true] {
            let traits = UITraitCollection(userInterfaceStyle: dark ? .dark : .light)
            let surface = UIColor(OBCTheme.surface).resolvedColor(with: traits)
            let ink = UIColor(OBCTheme.ink).resolvedColor(with: traits)
            let outline = UIColor(OBCTheme.secondary).resolvedColor(with: traits)
            for category in PlannerPreviewPlaceCategory.allCases {
                let image = UIGraphicsImageRenderer(size: CGSize(width: 28, height: 28)).image { _ in
                    let circle = UIBezierPath(ovalIn: CGRect(x: 1, y: 1, width: 26, height: 26))
                    surface.setFill(); circle.fill()
                    outline.setStroke(); circle.lineWidth = 1.5; circle.stroke()
                    let symbol = UIImage(systemName: category.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold))?
                        .withTintColor(ink, renderingMode: .alwaysOriginal)
                    if let symbol { symbol.draw(at: CGPoint(x: 14 - symbol.size.width / 2, y: 14 - symbol.size.height / 2)) }
                }
                style.setImage(image, forName: "poi-\(category.rawValue)-\(dark ? "dark" : "light")")
            }
        }
    }

    func draw(_ lines: [MapStroke], force: Bool = false) {
        guard let style else { return }
        installPOIImages(style)
        guard force || strokes != lines else { return }
        strokes = lines
        for layer in style.layers where layer.identifier.hasPrefix("obc-line-") { style.removeLayer(layer) }
        for source in style.sources where source.identifier.hasPrefix("obc-line-") { style.removeSource(source) }
        var groups: [[MapStroke]] = []
        for line in lines where line.coordinates.count > 1 {
            if let index = groups.firstIndex(where: { group in
                let first = group[0]
                return first.color == line.color && first.width == line.width && first.cased == line.cased && first.casingColor == line.casingColor && first.dash == line.dash
            }) { groups[index].append(line) } else { groups.append([line]) }
        }
        for (index, group) in groups.enumerated() {
            let line = group[0]
            let shapes = group.map { stroke -> MLNPolyline in
                var coordinates = MapGeometry.clLocations(stroke.coordinates)
                return MLNPolyline(coordinates: &coordinates, count: UInt(coordinates.count))
            }
            let shape = MLNShapeCollection(shapes: shapes)
            let id = "obc-line-\(index)"
            let source = MLNShapeSource(identifier: id, shape: shape, options: [.simplificationTolerance: 0.375])
            style.addSource(source)
            if line.cased {
                let casing = MLNLineStyleLayer(identifier: id + "-case", source: source)
                casing.lineColor = NSExpression(forConstantValue: UIColor(line.casingColor).resolvedColor(with: traitCollection))
                casing.lineWidth = NSExpression(forConstantValue: line.width + 3.6)
                casing.lineJoin = NSExpression(forConstantValue: "round")
                casing.lineCap = NSExpression(forConstantValue: "round")
                style.addLayer(casing)
            }
            let layer = MLNLineStyleLayer(identifier: id, source: source)
            layer.lineColor = NSExpression(forConstantValue: UIColor(line.color).resolvedColor(with: traitCollection))
            layer.lineWidth = NSExpression(forConstantValue: line.width)
            layer.lineJoin = NSExpression(forConstantValue: "round")
            layer.lineCap = NSExpression(forConstantValue: "round")
            if !line.dash.isEmpty { layer.lineDashPattern = NSExpression(forConstantValue: line.dash) }
            style.addLayer(layer)
        }
    }

    func fit(_ points: [Coordinate], bottom: CGFloat = 0, animated: Bool = false) {
        guard !points.isEmpty, bounds.width > 0, bounds.height > bottom + 80 else { return }
        let region = MapGeometry.boundingRegion(for: points)
        let sw = CLLocationCoordinate2D(latitude: region.center.latitude - region.span.latitudeDelta / 2,
                                       longitude: region.center.longitude - region.span.longitudeDelta / 2)
        let ne = CLLocationCoordinate2D(latitude: region.center.latitude + region.span.latitudeDelta / 2,
                                       longitude: region.center.longitude + region.span.longitudeDelta / 2)
        setVisibleCoordinateBounds(MLNCoordinateBounds(sw: sw, ne: ne),
                                   edgePadding: UIEdgeInsets(top: 30, left: 24, bottom: bottom + 30, right: 24),
                                   animated: animated, completionHandler: nil)
    }

    var mapRect: MKMapRect {
        let sw = MKMapPoint(visibleCoordinateBounds.sw), ne = MKMapPoint(visibleCoordinateBounds.ne)
        return MKMapRect(x: sw.x, y: ne.y, width: ne.x - sw.x, height: sw.y - ne.y)
    }
}

struct OBCMapView: UIViewRepresentable {
    var lines: [MapStroke]
    var pins: [MapPin] = []
    var interactive = true
    var bottomInset: CGFloat = 0
    var fitRevision = 0
    var release: PlannerRelease?
    var online = true
    var showsScale = false
    var onTap: ((Coordinate, CGPoint, OBCNativeMapView) -> Void)?
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.obcPlannerSource) private var plannerSource

    func makeUIView(context: Context) -> OBCNativeMapView {
        let map = OBCNativeMapView()
        map.delegate = context.coordinator
        map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
            guard let map, let coordinator else { return }
            map.fit(coordinator.parent.fitPoints, bottom: coordinator.parent.bottomInset)
        }
        map.addGestureRecognizer(UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped)))
        return map
    }
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func updateUIView(_ map: OBCNativeMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        map.isUserInteractionEnabled = interactive
        map.showsScale = showsScale
        map.load(release: release, dark: colorScheme == .dark, online: online, source: plannerSource)
        map.draw(lines)
        coordinator.updatePins(map)
        let geometry = lines.map(\.coordinates)
        if coordinator.fitRevision != fitRevision || (!interactive && coordinator.geometry != geometry) {
            coordinator.geometry = geometry
            coordinator.fitRevision = fitRevision
            map.fit(fitPoints, bottom: bottomInset)
        }
    }
    static func dismantleUIView(_ map: OBCNativeMapView, coordinator: Coordinator) { map.stop() }
    private var fitPoints: [Coordinate] { lines.flatMap(\.coordinates) + pins.map(\.coordinate) }

    @MainActor final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var parent: OBCMapView
        var fitRevision = -1
        var geometry: [[Coordinate]] = []
        private var pins: [MapPin] = []
        private var appearance: UIUserInterfaceStyle?
        init(_ parent: OBCMapView) { self.parent = parent }
        func updatePins(_ map: OBCNativeMapView) {
            guard pins != parent.pins || appearance != map.traitCollection.userInterfaceStyle else { return }
            appearance = map.traitCollection.userInterfaceStyle
            pins = parent.pins
            map.removeAnnotations(map.annotations ?? [])
            map.addAnnotations(pins.map { pin in
                let annotation = NativePin()
                annotation.pin = pin
                annotation.coordinate = MapGeometry.clLocation(pin.coordinate)
                annotation.title = pin.label.isEmpty ? "Route point" : pin.label
                return annotation
            })
        }
        func mapView(_ mapView: MLNMapView, regionDidChangeAnimated animated: Bool) {
            (mapView as? OBCNativeMapView)?.updateCoverageStatus()
        }
        func mapViewDidFinishLoadingMap(_ mapView: MLNMapView) {
            (mapView as? OBCNativeMapView)?.didFinishLoadingMap()
        }
        func mapViewDidFailLoadingMap(_ mapView: MLNMapView, withError error: Error) {
            (mapView as? OBCNativeMapView)?.didFailLoadingMap()
        }
        func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
            (mapView as? OBCNativeMapView)?.draw(parent.lines, force: true)
        }
        func mapView(_ mapView: MLNMapView, viewFor annotation: any MLNAnnotation) -> MLNAnnotationView? {
            guard let annotation = annotation as? NativePin else { return nil }
            let view = mapView.dequeueReusableAnnotationView(withIdentifier: "point") ?? MLNAnnotationView(reuseIdentifier: "point")
            view.annotation = annotation
            let pin = annotation.pin
            let drawing: AnyView
            if let highlighted = pin.photo { drawing = AnyView(PhotoPin(highlighted: highlighted)) }
            else if !pin.label.isEmpty { drawing = AnyView(WaypointPinBadge(label: pin.label)) }
            else if let symbol = pin.symbol {
                drawing = AnyView(Image(systemName: symbol).font(.system(.caption2, weight: .semibold))
                    .foregroundStyle(pin.color).frame(width: 20, height: 20)
                    .background(Circle().fill(OBCTheme.surface)).overlay(Circle().strokeBorder(OBCTheme.hairlineStrong)))
            } else if pin.square {
                drawing = AnyView(RoundedRectangle(cornerRadius: 2).fill(pin.color).frame(width: pin.size, height: pin.size)
                    .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(OBCTheme.surface, lineWidth: 2)))
            } else {
                drawing = AnyView(Circle().fill(pin.color).frame(width: pin.size, height: pin.size)
                    .overlay(Circle().strokeBorder(OBCTheme.surface, lineWidth: 2)))
            }
            let renderer = ImageRenderer(content: drawing.environment(\.colorScheme, mapView.traitCollection.userInterfaceStyle == .dark ? .dark : .light))
            renderer.scale = mapView.traitCollection.displayScale
            let image = renderer.uiImage
            view.subviews.forEach { $0.removeFromSuperview() }
            let imageView = UIImageView(image: image)
            view.frame.size = image?.size ?? .zero
            imageView.frame = view.bounds
            view.addSubview(imageView)
            view.isUserInteractionEnabled = false
            return view
        }
        @objc func tapped(_ recognizer: UITapGestureRecognizer) {
            guard let map = recognizer.view as? OBCNativeMapView else { return }
            let point = recognizer.location(in: map), coordinate = map.convert(point, toCoordinateFrom: map)
            parent.onTap?(Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude), point, map)
        }
    }
}

private final class NativePin: MLNPointAnnotation { var pin = MapPin(coordinate: Coordinate(latitude: 0, longitude: 0)) }
#endif
