import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#if os(iOS)
import MapLibre
#endif
#endif

/// Every filtered ride on one map in the ride colour over a light halo. A tap on a line
/// emphasises that ride and quiets the others, and its card opens it. Where rides overlap, the
/// rider picks one.
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
                        .background(OBCTheme.page.opacity(0.96))
                        .overlay(alignment: .bottom) {
                            Rectangle().fill(OBCTheme.hairline).frame(height: 1)
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
                .obcChoiceSheet("Which ride?", isPresented: choicesShown, actions: choices.compactMap { id in
                    ride(id).map { ride in OBCSheetAction("\(ride.name) · \(OBCFormat.rideDay(ride.date))") { select(id) } }
                })
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
        .accessibilityLabel(model.filteredRides.count == 1 ? "Map of 1 ride" : "Map of \(model.filteredRides.count) rides")
        #else
        OBCTheme.page
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
                    .fill(OBCTheme.ride)
                    .frame(width: 6, height: 40)
                VStack(alignment: .leading, spacing: 4) {
                    Text(ride.name)
                        .font(.system(.callout, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(2)
                    Text([
                        ride.date.formatted(date: .abbreviated, time: .omitted),
                        OBCFormat.distance(meters: ride.distanceMeters),
                        ride.bikeType.name,
                    ].joined(separator: " · "))
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .lineLimit(2)
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.system(.subheadline, weight: .semibold))
                    .foregroundStyle(OBCTheme.secondary)
            }
            .padding(14)
            .background(
                RoundedRectangle(cornerRadius: OBCTheme.radiusCard)
                    .fill(OBCTheme.surface)
                    .shadow(color: .black.opacity(0.18), radius: 14, y: 4)
            )
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 16)
        // Clears the map's legal link, which must stay visible.
        .padding(.bottom, 58)
        .accessibilityIdentifier("libraryMap.rideCard")
    }
}

#if os(iOS)
/// The map itself. UIKit, because rides use prepared detail levels, and the
/// lines change level with the zoom.
struct RideLinesMap: UIViewRepresentable {
    let lines: RideMapLines?
    let selected: RideID?
    let isOnline: Bool
    /// Every ride within 20 pt of the tap, nearest first.
    let onTap: ([RideID]) -> Void

    static let tapRadiusPoints = 20.0

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.obcPlannerSource) private var plannerSource

    func makeCoordinator() -> Coordinator { Coordinator() }
    static func dismantleUIView(_ map: OBCNativeMapView, coordinator: Coordinator) { map.stop() }

    func makeUIView(context: Context) -> OBCNativeMapView {
        let map = OBCNativeMapView()
        map.delegate = context.coordinator
        map.onFirstLayout = { [weak map, weak coordinator = context.coordinator] in
            guard let map, let coordinator else { return }; coordinator.update(map, lines: lines, selected: selected, isOnline: isOnline)
        }
        map.addGestureRecognizer(
            UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped)))
        return map
    }

    func updateUIView(_ map: OBCNativeMapView, context: Context) {
        map.load(dark: colorScheme == .dark, online: isOnline, source: plannerSource)
        context.coordinator.onTap = onTap
        context.coordinator.update(map, lines: lines, selected: selected, isOnline: isOnline)
    }

    @MainActor
    final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var onTap: ([RideID]) -> Void = { _ in }
        private var lines: RideMapLines?
        private var drawn: (ids: [RideID], selected: RideID?, level: Int)?
        private var fitted = false

        func update(_ map: OBCNativeMapView, lines: RideMapLines?, selected: RideID?, isOnline: Bool) {
            self.lines = lines
            self.selection = selected
            if !fitted, let lines, map.bounds.width > 0 {
                fit(map, to: lines)
            }
            redraw(map, selected: selected)
        }

        func mapViewDidFinishLoadingMap(_ mapView: MLNMapView) {
            (mapView as? OBCNativeMapView)?.didFinishLoadingMap()
        }
        func mapViewDidFailLoadingMap(_ mapView: MLNMapView, withError error: Error) {
            (mapView as? OBCNativeMapView)?.didFailLoadingMap()
        }
        func mapView(_ mapView: MLNMapView, regionDidChangeAnimated animated: Bool) {
            (mapView as? OBCNativeMapView)?.updateCoverageStatus()
            guard let map = mapView as? OBCNativeMapView else { return }
            if !fitted, let lines { fit(map, to: lines) }
            redraw(map, selected: selection)
        }

        func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
            drawn = nil
            if let map = mapView as? OBCNativeMapView { redraw(map, selected: selection, force: true) }
        }
        private var selection: RideID?

        @objc func tapped(_ gesture: UITapGestureRecognizer) {
            guard let map = gesture.view as? OBCNativeMapView, let lines else { return }
            let coordinate = map.convert(gesture.location(in: map), toCoordinateFrom: map)
            let metersPerPoint = Self.metersPerPoint(map)
            onTap(lines.rides(
                near: Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude),
                withinMeters: RideLinesMap.tapRadiusPoints * metersPerPoint,
                metersPerPoint: metersPerPoint
            ))
        }

        private func redraw(_ map: OBCNativeMapView, selected: RideID?, force: Bool = false) {
            guard let lines, map.bounds.width > 0 else { return }
            let metersPerPoint = Self.metersPerPoint(map)
            let visible = lines.lines(metersPerPoint: metersPerPoint)
            let state = (ids: visible.map(\.id), selected: selected, level: lines.level(metersPerPoint: metersPerPoint))
            if !force, let drawn, drawn.ids == state.ids, drawn.selected == state.selected, drawn.level == state.level {
                return
            }
            drawn = state
            let others = visible.filter { $0.id != selected }
            let chosen = visible.filter { $0.id == selected }
            let quiet = selected != nil
            let strokes = others.flatMap { line in line.pieces.map {
                MapStroke(coordinates: $0, color: OBCTheme.ride.opacity(quiet ? 0.35 : 1), width: 2.6, casingColor: OBCTheme.surface.opacity(quiet ? 0.5 : 1))
            } } + chosen.flatMap { line in line.pieces.map {
                MapStroke(coordinates: $0, color: OBCTheme.ride, width: 4.5, casingColor: OBCTheme.surface)
            } }
            map.draw(strokes, force: force)

        }

        private func fit(_ map: OBCNativeMapView, to lines: RideMapLines) {
            let coordinates = lines.lines(metersPerPoint: .infinity).flatMap { $0.pieces.joined() }
            guard !coordinates.isEmpty else { return }
            fitted = true
            map.fit(coordinates, bottom: 100)

        }

        private static func metersPerPoint(_ map: OBCNativeMapView) -> Double {
            map.metersPerPoint(atLatitude: map.centerCoordinate.latitude)
        }
    }
}

#endif
