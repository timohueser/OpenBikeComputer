#if os(iOS)
import MapLibre
import OBCDomain
import OBCPlanner
import SwiftUI

struct OfflineAreaView: View {
    @Bindable var model: OfflineMapsModel
    let initialBounds: [Double]?
    let onReview: () -> Void
    @Environment(\.obcPlannerSource) private var source
    @State private var mode = 0
    @State private var selection: [Double]?
    @State private var focus: [Double]?
    @State private var point: Coordinate?
    @State private var region: OfflineRegion?
    @State private var query = ""
    @State private var places: [PlannerPlace] = []
    @State private var searching = false
    @State private var name = "Map area"

    var body: some View {
        GeometryReader { geometry in
            VStack(spacing: 0) {
                HStack {
                    Image(systemName: "magnifyingglass")
                    TextField("Find an area", text: $query).autocorrectionDisabled()
                        .accessibilityIdentifier("offline.search")
                    if searching { ProgressView() }
                }.padding(12).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                    .padding(.horizontal, 16).padding(.bottom, 10)
                Picker("Selection", selection: $mode) {
                    Text("Select on map").tag(0); Text("Choose a region").tag(1)
                }.pickerStyle(.segmented).padding(.horizontal, 16).padding(.bottom, 12)
                OfflineAreaMap(focus: focus ?? initialBounds, region: mode == 1 ? region : nil, box: mode == 0,
                    onBounds: { selection = $0 }, onPoint: { point = $0; region = nil })
                    .frame(height: max(190, min(340, geometry.size.height * 0.46)))
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        if !query.isEmpty {
                            ForEach(places) { place in
                                Button {
                                    name = place.name + " area"
                                    focus = [place.lon - 0.15, place.lat - 0.1, place.lon + 0.15, place.lat + 0.1]
                                    point = place.coordinate; query = ""; places = []
                                } label: {
                                    HStack { Text(place.name); Spacer(); Text(place.city).foregroundStyle(OBCTheme.secondary) }
                                        .frame(minHeight: 44)
                                }
                            }
                        }
                        if mode == 1 {
                            Text(point == nil ? "Tap a place on the map" : "Regions at this point").font(.headline)
                            ForEach(candidates) { option in
                                Button {
                                    region = option; focus = option.bounds; name = option.name
                                } label: {
                                    HStack {
                                        VStack(alignment: .leading, spacing: 3) {
                                            Text(option.name)
                                            Text(option.available ? parentName(option) : "Not available for offline planning")
                                                .font(.footnote).foregroundStyle(OBCTheme.secondary)
                                        }
                                        Spacer()
                                        if region?.id == option.id { Image(systemName: "checkmark") }
                                    }.frame(minHeight: 44)
                                }.disabled(!option.available)
                            }
                            if region != nil {
                                Text("The download includes the rectangle around this region.")
                                    .font(.footnote).foregroundStyle(OBCTheme.secondary)
                            }
                        } else {
                            Text("Move and zoom the map to fit your area.").foregroundStyle(OBCTheme.secondary)
                        }
                        TextField("Map name", text: $name).textFieldStyle(.roundedBorder)
                            .accessibilityIdentifier("offline.name")
                        if let error = model.error { Text(error).foregroundStyle(OBCTheme.danger) }
                        if model.isBusy {
                            ProgressView(model.status)
                            Button("Cancel preparation") { model.stop() }.buttonStyle(.obcGhost)
                        } else {
                            Text("We prepare the map and check its size before you download it.")
                                .font(.footnote).foregroundStyle(OBCTheme.secondary)
                            Button("Review download") {
                                guard let bounds = mode == 1 ? region?.bounds : selection else { return }
                                model.prepare(bounds: bounds, region: mode == 1 ? region?.id : nil,
                                              name: name.trimmingCharacters(in: .whitespacesAndNewlines))
                            }.buttonStyle(.obcPrimary)
                                .disabled((mode == 1 ? region?.bounds : selection) == nil || name.trimmingCharacters(in: .whitespaces).isEmpty)
                                .accessibilityIdentifier("offline.prepare")
                        }
                    }.padding(20)
                }.scrollBounceBehavior(.basedOnSize)
            }
        }
        .background(OBCTheme.page).foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
        .navigationTitle("Select area").navigationBarTitleDisplayMode(.inline)
        .task { await model.loadRegions() }
        .task(id: query) {
            places = []
            guard query.count > 1 else { return }
            searching = true
            defer { searching = false }
            do {
                try await Task.sleep(for: .milliseconds(350))
                let release = try await source.release()
                let results = try await source.search(PlannerSearchQuery(text: query, view: selection), release: release)
                try Task.checkCancellation()
                places = Array(results.prefix(5))
            } catch is CancellationError {} catch { model.error = error.localizedDescription }
        }
        .onChange(of: model.quote?.map.id) { _, value in if value != nil { onReview() } }
        .onChange(of: mode) { _, _ in model.error = nil }
    }

    private var candidates: [OfflineRegion] {
        model.regions.filter { region in
            if !query.isEmpty { return region.name.localizedCaseInsensitiveContains(query) }
            guard let point else { return false }
            return region.contains(longitude: point.longitude, latitude: point.latitude)
        }.sorted { ($0.bounds[2] - $0.bounds[0]) * ($0.bounds[3] - $0.bounds[1])
            < ($1.bounds[2] - $1.bounds[0]) * ($1.bounds[3] - $1.bounds[1]) }
    }
    private func parentName(_ option: OfflineRegion) -> String {
        model.regions.first { $0.id == option.parent }?.name ?? "Available region"
    }
}

private struct OfflineAreaMap: UIViewRepresentable {
    var focus: [Double]?
    var region: OfflineRegion?
    var box: Bool
    let onBounds: ([Double]) -> Void
    let onPoint: (Coordinate) -> Void
    @Environment(\.obcPlannerSource) private var source
    @Environment(\.colorScheme) private var colorScheme

    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeUIView(context: Context) -> OBCNativeMapView {
        let map = OBCNativeMapView()
        map.delegate = context.coordinator
        map.accessibilityIdentifier = "offline.areaMap"
        context.coordinator.frame.isUserInteractionEnabled = false
        context.coordinator.frame.layer.borderWidth = 2
        map.addSubview(context.coordinator.frame)
        map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
            guard let map, let coordinator else { return }
            coordinator.focus(map); coordinator.changed(map)
        }
        map.addGestureRecognizer(UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tap(_:))))
        return map
    }
    func updateUIView(_ map: OBCNativeMapView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        map.load(dark: colorScheme == .dark, source: source)
        coordinator.frame.layer.borderColor = UIColor(OBCTheme.secondary).cgColor
        coordinator.frame.isHidden = !box
        if coordinator.lastFocus != focus { coordinator.focus(map) }
        let lines = region?.rings.map { ring in
            MapStroke(coordinates: ring.filter { $0.count == 2 }.map { Coordinate(latitude: $0[1], longitude: $0[0]) },
                      color: OBCTheme.secondary, width: 2, cased: false)
        } ?? []
        map.draw(lines)
    }
    static func dismantleUIView(_ map: OBCNativeMapView, coordinator: Coordinator) { map.stop() }

    @MainActor final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var parent: OfflineAreaMap
        var lastFocus: [Double]?
        let frame = UIView()
        init(_ parent: OfflineAreaMap) { self.parent = parent }
        func focus(_ map: OBCNativeMapView) {
            guard map.bounds.width > 0, let bounds = parent.focus, OfflineMap.valid(bounds) else { return }
            lastFocus = bounds
            map.setVisibleCoordinateBounds(.init(sw: .init(latitude: bounds[1], longitude: bounds[0]),
                                                 ne: .init(latitude: bounds[3], longitude: bounds[2])),
                edgePadding: UIEdgeInsets(top: 36, left: 28, bottom: 36, right: 28), animated: false)
        }
        func changed(_ map: OBCNativeMapView) {
            guard map.bounds.width > 0, map.bounds.height > 0 else { return }
            frame.frame = map.bounds.insetBy(dx: 28, dy: 36)
            let nw = map.convert(CGPoint(x: frame.frame.minX, y: frame.frame.minY), toCoordinateFrom: map)
            let se = map.convert(CGPoint(x: frame.frame.maxX, y: frame.frame.maxY), toCoordinateFrom: map)
            let bounds = [nw.longitude, se.latitude, se.longitude, nw.latitude]
            if OfflineMap.valid(bounds) { parent.onBounds(bounds) }
        }
        func mapView(_ map: MLNMapView, regionDidChangeAnimated animated: Bool) {
            if let map = map as? OBCNativeMapView { changed(map); map.updateCoverageStatus() }
        }
        func mapViewDidFinishLoadingMap(_ map: MLNMapView) {
            guard let map = map as? OBCNativeMapView else { return }
            map.didFinishLoadingMap(); changed(map)
        }
        func mapViewDidFailLoadingMap(_ map: MLNMapView, withError error: Error) { (map as? OBCNativeMapView)?.didFailLoadingMap() }
        @objc func tap(_ gesture: UITapGestureRecognizer) {
            guard let map = gesture.view as? OBCNativeMapView, !parent.box else { return }
            let coordinate = map.convert(gesture.location(in: map), toCoordinateFrom: map)
            parent.onPoint(Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude))
        }
    }
}
#endif
