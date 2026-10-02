#if os(iOS)
import MapLibre
import OBCDomain
import OBCPlanner
import SwiftUI

struct OfflineAreaMap: UIViewRepresentable {
    @Binding var selection: OfflineAreaSelection?
    var coverage: OfflineCoverage?
    var fitRevision = 0
    var editable = true
    @Environment(\.obcPlannerSource) private var source
    @Environment(\.colorScheme) private var colorScheme

    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeUIView(context: Context) -> OfflineAreaCanvas {
        let coordinator = context.coordinator
        let canvas = OfflineAreaCanvas(selection: coordinator.frame)
        canvas.map.delegate = coordinator
        canvas.map.accessibilityIdentifier = "offline.areaMap"
        coordinator.frame.onResize = { [weak coordinator, weak map = canvas.map] corner, point in
            guard let coordinator, let map, var selection = coordinator.parent.selection else { return }
            let coordinate = map.convert(point, toCoordinateFrom: map)
            selection.move(corner, to: Coordinate(latitude: coordinate.latitude, longitude: coordinate.longitude))
            coordinator.parent.selection = selection
            coordinator.updateCoverage()
            coordinator.render(map)
        }
        canvas.onLayout = { [weak coordinator] map in
            coordinator?.fitIfNeeded(map)
            coordinator?.render(map)
        }
        return canvas
    }

    func updateUIView(_ canvas: OfflineAreaCanvas, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        canvas.map.load(dark: colorScheme == .dark, source: source)
        coordinator.updateCoverage()
        coordinator.fitIfNeeded(canvas.map)
        coordinator.render(canvas.map)
    }

    static func dismantleUIView(_ canvas: OfflineAreaCanvas, coordinator: Coordinator) { canvas.map.stop() }

    @MainActor final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var parent: OfflineAreaMap
        let frame = OfflineSelectionFrame()
        private var lastFitRevision: Int?
        private var coverageBounds: [Double]?

        init(_ parent: OfflineAreaMap) { self.parent = parent }

        func updateCoverage() {
            guard let bounds = parent.selection?.bounds,
                  let cells = parent.coverage?.cells(covering: bounds), let first = cells.first else {
                coverageBounds = nil
                return
            }
            coverageBounds = cells.reduce(first) { result, cell in
                [min(result[0], cell[0]), min(result[1], cell[1]), max(result[2], cell[2]), max(result[3], cell[3])]
            }
        }

        func fitIfNeeded(_ map: OBCNativeMapView) {
            guard map.bounds.width > 0, map.bounds.height > 0,
                  lastFitRevision != parent.fitRevision, let bounds = parent.selection?.bounds else { return }
            lastFitRevision = parent.fitRevision
            let inset: CGFloat = parent.editable ? 52 : 20
            map.setVisibleCoordinateBounds(
                .init(sw: .init(latitude: bounds[1], longitude: bounds[0]), ne: .init(latitude: bounds[3], longitude: bounds[2])),
                edgePadding: UIEdgeInsets(top: inset, left: inset, bottom: inset, right: inset),
                animated: false, completionHandler: nil)
        }

        func render(_ map: OBCNativeMapView) {
            func rectangle(_ bounds: [Double]) -> CGRect {
                let nw = map.convert(.init(latitude: bounds[3], longitude: bounds[0]), toPointTo: map)
                let se = map.convert(.init(latitude: bounds[1], longitude: bounds[2]), toPointTo: map)
                return CGRect(x: nw.x, y: nw.y, width: se.x - nw.x, height: se.y - nw.y)
            }
            frame.show(selection: parent.selection.map { rectangle($0.bounds) },
                       coverage: coverageBounds.map(rectangle), editable: parent.editable)
        }

        func mapViewRegionIsChanging(_ map: MLNMapView) {
            if let map = map as? OBCNativeMapView { render(map) }
        }
        func mapView(_ map: MLNMapView, regionDidChangeAnimated animated: Bool) {
            if let map = map as? OBCNativeMapView {
                render(map)
                map.updateCoverageStatus()
                map.viewportSettled()
            }
        }
        func mapViewDidFinishLoadingMap(_ map: MLNMapView) {
            guard let map = map as? OBCNativeMapView else { return }
            map.didFinishLoadingMap()
            render(map)
        }
        func mapViewDidFailLoadingMap(_ map: MLNMapView, withError error: Error) {
            (map as? OBCNativeMapView)?.didFailLoadingMap()
        }
    }
}

final class OfflineAreaCanvas: UIView {
    let map = OBCNativeMapView()
    private let selection: OfflineSelectionFrame
    var onLayout: ((OBCNativeMapView) -> Void)?

    init(selection: OfflineSelectionFrame) {
        self.selection = selection
        super.init(frame: .zero)
        clipsToBounds = true
        addSubview(map)
        selection.attach(to: map)
        accessibilityElements = [map, selection]
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func layoutSubviews() {
        super.layoutSubviews()
        map.frame = bounds
        selection.frame = map.bounds
        map.layoutIfNeeded()
        onLayout?(map)
    }
}
#endif
