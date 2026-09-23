import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// Every filtered ride on one map, in one colour over a light halo, so overlapping rides build up.
/// A tap on a line highlights that ride, and its card opens it. Where rides overlap, the rider
/// picks one.
public struct RideLibraryMapView: View {
    private let model: RideLibraryModel
    private let onOpenRide: (RideSummary) -> Void
    private let onClose: () -> Void

    @Environment(\.obcIsOnline) private var isOnline
    @State private var selected: RideID?
    @State private var choices: [RideID] = []

    public init(
        model: RideLibraryModel,
        onOpenRide: @escaping (RideSummary) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.model = model
        self.onOpenRide = onOpenRide
        self.onClose = onClose
    }

    public var body: some View {
        NavigationStack {
            map
                .ignoresSafeArea(edges: .bottom)
                .safeAreaInset(edge: .top, spacing: 0) {
                    RideFilterBar(model: model)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                        .background(OBCTheme.parchment.opacity(0.96))
                        .overlay(alignment: .bottom) {
                            Rectangle().fill(OBCTheme.line).frame(height: 1)
                        }
                }
                .overlay(alignment: .bottom) {
                    if let ride = selectedRide {
                        rideCard(ride)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }
                }
                .navigationTitle("All rides")
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done", action: onClose).fontWeight(.semibold)
                    }
                }
                .confirmationDialog("Which ride?", isPresented: choicesShown, titleVisibility: .visible) {
                    ForEach(choices, id: \.self) { id in
                        if let ride = ride(id) {
                            Button("\(ride.name) · \(OBCFormat.rideDay(ride.date))") { select(id) }
                        }
                    }
                }
                .accessibilityIdentifier("libraryMap.screen")
        }
        .tint(OBCTheme.tint)
        .onChange(of: model.filteredRides.map(\.id)) { _, ids in
            if let selected, !ids.contains(selected) { self.selected = nil }
        }
    }

    @ViewBuilder
    private var map: some View {
        #if canImport(UIKit) && canImport(MapKit)
        RideLinesMap(lines: model.filteredMapLines, selected: selected, isOnline: isOnline) { hits in
            switch hits.count {
            case 0: select(nil)
            case 1: select(hits[0])
            default: choices = hits
            }
        }
        #else
        OBCTheme.parchment
        #endif
    }

    private var selectedRide: RideSummary? { selected.flatMap(ride) }

    private var choicesShown: Binding<Bool> {
        Binding(get: { !choices.isEmpty }, set: { if !$0 { choices = [] } })
    }

    private func ride(_ id: RideID) -> RideSummary? {
        model.filteredRides.first { $0.id == id }
    }

    private func select(_ id: RideID?) {
        withAnimation(.snappy(duration: 0.22)) { selected = id }
    }

    private func rideCard(_ ride: RideSummary) -> some View {
        Button {
            onOpenRide(ride)
        } label: {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 3)
                    .fill(OBCTheme.coral)
                    .frame(width: 6, height: 40)
                VStack(alignment: .leading, spacing: 4) {
                    Text(ride.name)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(1)
                    Text([
                        ride.date.formatted(date: .abbreviated, time: .omitted),
                        OBCFormat.distance(meters: ride.distanceMeters),
                        ride.bikeType.name,
                    ].joined(separator: " · "))
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(OBCTheme.forest)
            }
            .padding(14)
            .background(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                    .fill(OBCTheme.panel)
                    .shadow(color: .black.opacity(0.18), radius: 14, y: 4)
            )
            .overlay(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).stroke(OBCTheme.line, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 16)
        .padding(.bottom, 34)
        .accessibilityIdentifier("libraryMap.rideCard")
    }
}

#if canImport(UIKit) && canImport(MapKit)
/// The map itself. UIKit, because 500 rides draw fast only as a few `MKMultiPolyline`s, and the
/// lines change level with the zoom.
struct RideLinesMap: UIViewRepresentable {
    let lines: RideMapLines?
    let selected: RideID?
    let isOnline: Bool
    /// Every ride within 20 pt of the tap, nearest first.
    let onTap: ([RideID]) -> Void

    static let tapRadiusPoints = 20.0

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> MKMapView {
        let map = MKMapView()
        map.delegate = context.coordinator
        map.overrideUserInterfaceStyle = .light
        map.showsCompass = true
        map.showsScale = true
        map.pointOfInterestFilter = .excludingAll
        map.addGestureRecognizer(
            UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped)))
        return map
    }

    func updateUIView(_ map: MKMapView, context: Context) {
        context.coordinator.onTap = onTap
        context.coordinator.update(map, lines: lines, selected: selected, isOnline: isOnline)
    }

    @MainActor
    final class Coordinator: NSObject, MKMapViewDelegate {
        var onTap: ([RideID]) -> Void = { _ in }
        private var lines: RideMapLines?
        private var drawn: (ids: [RideID], selected: RideID?, level: Int)?
        private var fitted = false
        private let grid = GridOverlay()

        func update(_ map: MKMapView, lines: RideMapLines?, selected: RideID?, isOnline: Bool) {
            self.lines = lines
            let showsGrid = map.overlays.contains { $0 === grid }
            if !isOnline, !showsGrid { map.addOverlay(grid, level: .aboveRoads) }
            if isOnline, showsGrid { map.removeOverlay(grid) }
            if !fitted, let lines, map.bounds.width > 0 {
                fit(map, to: lines)
            }
            redraw(map, selected: selected)
        }

        func mapView(_ map: MKMapView, regionDidChangeAnimated animated: Bool) {
            if !fitted, let lines { fit(map, to: lines) }
            redraw(map, selected: drawn?.selected)
        }

        func mapView(_ mapView: MKMapView, rendererFor overlay: MKOverlay) -> MKOverlayRenderer {
            if overlay === grid { return GridRenderer(overlay: overlay) }
            guard let line = overlay as? StyledMultiPolyline else { return MKOverlayRenderer(overlay: overlay) }
            let renderer = MKMultiPolylineRenderer(multiPolyline: line)
            renderer.strokeColor = UIColor(line.color)
            renderer.lineWidth = line.width
            renderer.lineCap = .round
            renderer.lineJoin = .round
            return renderer
        }

        @objc func tapped(_ gesture: UITapGestureRecognizer) {
            guard let map = gesture.view as? MKMapView, let lines else { return }
            let coordinate = map.convert(gesture.location(in: map), toCoordinateFrom: map)
            let metersPerPoint = Self.metersPerPoint(map)
            onTap(lines.rides(
                near: Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude),
                withinMeters: RideLinesMap.tapRadiusPoints * metersPerPoint,
                metersPerPoint: metersPerPoint
            ))
        }

        private func redraw(_ map: MKMapView, selected: RideID?) {
            guard let lines, map.bounds.width > 0 else { return }
            let metersPerPoint = Self.metersPerPoint(map)
            let visible = lines.lines(metersPerPoint: metersPerPoint)
            let state = (ids: visible.map(\.id), selected: selected, level: lines.level(metersPerPoint: metersPerPoint))
            if let drawn, drawn.ids == state.ids, drawn.selected == state.selected, drawn.level == state.level {
                return
            }
            drawn = state
            map.removeOverlays(map.overlays.filter { $0 is StyledMultiPolyline })
            let others = visible.filter { $0.id != selected }
            let chosen = visible.filter { $0.id == selected }
            map.addOverlays([
                StyledMultiPolyline(others, color: OBCTheme.trackHalo, width: 6),
                StyledMultiPolyline(others, color: OBCTheme.trackStroke, width: 2.6),
                StyledMultiPolyline(chosen, color: OBCTheme.trackHalo, width: 8),
                StyledMultiPolyline(chosen, color: OBCTheme.coral, width: 4),
            ].compactMap { $0 }, level: .aboveLabels)
        }

        private func fit(_ map: MKMapView, to lines: RideMapLines) {
            let points = lines.lines(metersPerPoint: .infinity)
                .flatMap { $0.pieces.joined() }
                .map { MKMapPoint(CLLocationCoordinate2D(latitude: $0.latitude, longitude: $0.longitude)) }
            guard !points.isEmpty else { return }
            let rect = points.reduce(MKMapRect.null) { $0.union(MKMapRect(origin: $1, size: MKMapSize(width: 1, height: 1))) }
            fitted = true
            map.setVisibleMapRect(rect, edgePadding: UIEdgeInsets(top: 60, left: 40, bottom: 140, right: 40), animated: false)
        }

        private static func metersPerPoint(_ map: MKMapView) -> Double {
            let rect = map.visibleMapRect
            return rect.width * MKMetersPerMapPointAtLatitude(map.region.center.latitude) / Double(map.bounds.width)
        }
    }
}

/// One stroke of many rides: the renderer reads its colour and width.
private final class StyledMultiPolyline: MKMultiPolyline {
    private(set) var color: Color = .clear
    private(set) var width: CGFloat = 0

    convenience init?(_ lines: [RideMapLine], color: Color, width: CGFloat) {
        let polylines = lines.flatMap { line in
            line.pieces.map { piece in
                MKPolyline(coordinates: piece.map {
                    CLLocationCoordinate2D(latitude: $0.latitude, longitude: $0.longitude)
                }, count: piece.count)
            }
        }
        guard !polylines.isEmpty else { return nil }
        self.init(polylines)
        self.color = color
        self.width = width
    }
}

/// The offline fallback: gridded parchment over the whole world, under the lines.
private final class GridOverlay: NSObject, MKOverlay {
    let coordinate = CLLocationCoordinate2D(latitude: 0, longitude: 0)
    let boundingMapRect = MKMapRect.world
}

private final class GridRenderer: MKOverlayRenderer {
    override func draw(_ mapRect: MKMapRect, zoomScale: MKZoomScale, in context: CGContext) {
        let rect = self.rect(for: mapRect)
        context.setFillColor(UIColor(OBCTheme.parchment2).cgColor)
        context.fill(rect)
        let step = 22 / zoomScale
        context.setStrokeColor(UIColor(OBCTheme.gridLine).cgColor)
        context.setLineWidth(1 / zoomScale)
        var x = (rect.minX / step).rounded(.down) * step
        while x <= rect.maxX {
            context.move(to: CGPoint(x: x, y: rect.minY))
            context.addLine(to: CGPoint(x: x, y: rect.maxY))
            x += step
        }
        var y = (rect.minY / step).rounded(.down) * step
        while y <= rect.maxY {
            context.move(to: CGPoint(x: rect.minX, y: y))
            context.addLine(to: CGPoint(x: rect.maxX, y: y))
            y += step
        }
        context.strokePath()
    }
}
#endif
