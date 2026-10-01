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
        @State private var selection: [Double]?
        @State private var focus: [Double]?
        @State private var query = ""
        @State private var places: [PlannerPlace] = []
        @State private var searching = false
        @State private var name = "Map area"

        var body: some View {
            VStack(spacing: 0) {
                HStack {
                    Image(systemName: "magnifyingglass")
                    TextField("Find an area", text: $query).autocorrectionDisabled()
                        .accessibilityIdentifier("offline.search")
                    if searching { ProgressView() }
                }.padding(12).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                    .padding(.horizontal, 16).padding(.bottom, 10)
                OfflineAreaMap(
                    focus: focus ?? initialBounds ?? model.coverage?.bounds, coverage: model.coverage,
                    onBounds: { selection = $0 }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .allowsHitTesting(!model.isBusy)
                .overlay(alignment: .top) {
                    if !query.isEmpty && !places.isEmpty {
                        ScrollView {
                            VStack(spacing: 0) {
                                ForEach(places) { place in
                                    Button {
                                        name = place.name + " area"
                                        focus = [place.lon - 0.15, place.lat - 0.1, place.lon + 0.15, place.lat + 0.1]
                                        query = ""
                                        places = []
                                    } label: {
                                        HStack {
                                            Text(place.name)
                                            Spacer()
                                            Text(place.city).foregroundStyle(OBCTheme.secondary)
                                        }
                                        .frame(minHeight: 44).padding(.horizontal, 16)
                                    }
                                    Divider()
                                }
                            }
                        }.frame(maxHeight: 220).background(OBCTheme.surface)
                    }
                }
                VStack(alignment: .leading, spacing: 12) {
                    if model.isBusy {
                        OfflinePreparationView(model: model)
                    } else {
                        Label("Drag an edge or corner to resize", systemImage: "arrow.up.left.and.arrow.down.right")
                            .font(.subheadline)
                        Text("Highlighted grid cells are included in your download. Move the map to position your box.")
                            .font(.footnote).foregroundStyle(OBCTheme.secondary)
                        if let selection, let coverage = model.coverage, !coverage.contains(selection) {
                            Text("Keep the box inside available map coverage.").font(.footnote).foregroundStyle(OBCTheme.danger)
                        }
                        HStack {
                            Text("Name").foregroundStyle(OBCTheme.secondary)
                            TextField("Map name", text: $name).multilineTextAlignment(.trailing)
                                .accessibilityLabel("Map name").accessibilityIdentifier("offline.name")
                        }.padding(12).background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
                        if let error = model.error {
                            Text(error).font(.footnote).foregroundStyle(OBCTheme.danger)
                            if model.coverage == nil {
                                Button("Try again") { Task { await model.loadCoverage() } }.buttonStyle(.obcGhost)
                            }
                        }
                        Button("Review download") {
                            guard let bounds = selection else { return }
                            model.prepare(
                                bounds: bounds,
                                name: name.trimmingCharacters(in: .whitespacesAndNewlines))
                        }.buttonStyle(.obcPrimary)
                            .disabled(
                                selection.map { model.coverage?.contains($0) != true } ?? true
                                    || name.trimmingCharacters(in: .whitespaces).isEmpty
                            )
                            .accessibilityIdentifier("offline.prepare")
                    }
                }.padding(16).background(OBCTheme.page)

            }
            .background(OBCTheme.page).foregroundStyle(OBCTheme.ink).tint(OBCTheme.tint)
            .navigationTitle("Select area").navigationBarTitleDisplayMode(.inline)
            .task { await model.loadCoverage() }
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
        }
    }

    struct OfflineAreaMap: UIViewRepresentable {
        var focus: [Double]?
        var coverage: OfflineCoverage?
        var selecting = true
        let onBounds: ([Double]) -> Void
        @Environment(\.obcPlannerSource) private var source
        @Environment(\.colorScheme) private var colorScheme

        func makeCoordinator() -> Coordinator { Coordinator(self) }
        func makeUIView(context: Context) -> OfflineAreaCanvas {
            let canvas = OfflineAreaCanvas(selection: context.coordinator.frame)
            let map = canvas.map
            map.delegate = context.coordinator
            map.accessibilityIdentifier = "offline.areaMap"
            let selection = context.coordinator.frame
            selection.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            selection.onChange = { [weak map, weak coordinator = context.coordinator] _ in
                guard let map, let coordinator else { return }
                coordinator.changed(map)
            }
            map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
                guard let map, let coordinator else { return }
                coordinator.focus(map)
                coordinator.changed(map)
            }
            return canvas
        }
        func updateUIView(_ canvas: OfflineAreaCanvas, context: Context) {
            let map = canvas.map
            let coordinator = context.coordinator
            coordinator.parent = self
            map.load(dark: colorScheme == .dark, source: source)
            coordinator.frame.isHidden = !selecting
            if coordinator.lastFocus != focus { coordinator.focus(map) }
            if !selecting, let focus {
                let w = focus[0]
                let s = focus[1]
                let e = focus[2]
                let n = focus[3]
                map.draw([
                    MapStroke(
                        coordinates: [
                            Coordinate(latitude: s, longitude: w), Coordinate(latitude: s, longitude: e),
                            Coordinate(latitude: n, longitude: e), Coordinate(latitude: n, longitude: w),
                            Coordinate(latitude: s, longitude: w),
                        ],
                        color: OBCTheme.tint, width: 2, cased: true)
                ])
            }
            coordinator.changed(map)

        }
        static func dismantleUIView(_ canvas: OfflineAreaCanvas, coordinator: Coordinator) { canvas.map.stop() }

        @MainActor final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
            var parent: OfflineAreaMap
            var lastFocus: [Double]?
            var lastSelection: [Double]?
            let frame = OfflineSelectionFrame()
            init(_ parent: OfflineAreaMap) { self.parent = parent }
            func focus(_ map: OBCNativeMapView) {
                guard map.bounds.width > 0, let bounds = parent.focus, OfflineMap.valid(bounds) else { return }
                lastFocus = bounds
                map.setVisibleCoordinateBounds(
                    .init(
                        sw: .init(latitude: bounds[1], longitude: bounds[0]),
                        ne: .init(latitude: bounds[3], longitude: bounds[2])),
                    edgePadding: UIEdgeInsets(top: 36, left: 28, bottom: 36, right: 28), animated: false, completionHandler: nil)
            }
            func changed(_ map: OBCNativeMapView) {
                guard map.bounds.width > 0, map.bounds.height > 0 else { return }
                frame.frame = map.bounds
                frame.layoutIfNeeded()
                let rectangle = frame.selection
                guard !rectangle.isEmpty else { return }
                let nw = map.convert(CGPoint(x: rectangle.minX, y: rectangle.minY), toCoordinateFrom: map)
                let se = map.convert(CGPoint(x: rectangle.maxX, y: rectangle.maxY), toCoordinateFrom: map)
                let bounds = [nw.longitude, se.latitude, se.longitude, nw.latitude]
                frame.coverageRects = (parent.coverage?.cells(covering: bounds) ?? []).map { cell in
                    let nw = map.convert(.init(latitude: cell[3], longitude: cell[0]), toPointTo: map)
                    let se = map.convert(.init(latitude: cell[1], longitude: cell[2]), toPointTo: map)
                    return CGRect(x: nw.x, y: nw.y, width: se.x - nw.x, height: se.y - nw.y)
                }
                if OfflineMap.valid(bounds), lastSelection != bounds {
                    lastSelection = bounds
                    let callback = parent.onBounds
                    Task { @MainActor in callback(bounds) }
                }
            }
            func mapView(_ map: MLNMapView, regionDidChangeAnimated animated: Bool) {
                if let map = map as? OBCNativeMapView {
                    changed(map)
                    map.updateCoverageStatus()
                    map.viewportSettled()
                }
            }
            func mapViewDidFinishLoadingMap(_ map: MLNMapView) {
                guard let map = map as? OBCNativeMapView else { return }
                map.didFinishLoadingMap()
                changed(map)
            }
            func mapViewDidFailLoadingMap(_ map: MLNMapView, withError error: Error) {
                (map as? OBCNativeMapView)?.didFailLoadingMap()
            }

        }
    }
    final class OfflineAreaCanvas: UIView {
        let map = OBCNativeMapView()
        private let selection: OfflineSelectionFrame
        init(selection: OfflineSelectionFrame) {
            self.selection = selection
            super.init(frame: .zero)
            clipsToBounds = true
            addSubview(map)
            addSubview(selection)
            accessibilityElements = [map, selection]
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func layoutSubviews() {
            super.layoutSubviews()
            map.frame = bounds
            selection.frame = bounds
        }
    }
#endif
