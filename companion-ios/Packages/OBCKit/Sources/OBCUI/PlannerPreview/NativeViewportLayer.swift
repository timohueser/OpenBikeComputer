#if os(iOS)
import Foundation
import MapLibre
import OBCPlanner

/// Retains one padded viewport. Native workers load its GeoJSON from a temporary file.
@MainActor
final class NativeViewportLayer {
    let identifier: String
    init(identifier: String) { self.identifier = identifier }
    var status: (String?) -> Void = { _ in }
    private struct Viewport {
        let bounds: [Double]
        let zoom: Int
        let key: String
        func contains(_ other: Viewport) -> Bool {
            key == other.key && zoom == other.zoom && other.bounds[0] >= bounds[0]
                && other.bounds[1] >= bounds[1] && other.bounds[2] <= bounds[2] && other.bounds[3] <= bounds[3]
        }
    }
    private var cached: (viewport: Viewport, url: URL)?
    private var failed: Viewport?
    private var pending: Viewport?
    private var task: Task<Void, Never>?
    private var files: [URL] = []

    func update(_ map: OBCNativeMapView, key: String?, release: PlannerRelease?, minimumZoom: Double = 0,
                maximumZoom: Double = 24, load: @escaping @Sendable ([Double], Double, PlannerRelease) async throws -> Data) {
        guard let key, let release, map.zoomLevel >= minimumZoom, map.zoomLevel < maximumZoom else {
            task?.cancel(); pending = nil; failed = nil
            clear(map)
            status(nil)
            return
        }
        let visible = map.visibleCoordinateBounds
        let bounds = [max(release.bounds[0], visible.sw.longitude), max(release.bounds[1], visible.sw.latitude),
                      min(release.bounds[2], visible.ne.longitude), min(release.bounds[3], visible.ne.latitude)]
        guard bounds[0] < bounds[2], bounds[1] < bounds[3] else {
            task?.cancel(); pending = nil; failed = nil
            clear(map)
            status(nil)
            return
        }
        let viewport = Viewport(bounds: bounds, zoom: Int(map.zoomLevel), key: release.id + key)
        if let cached, cached.viewport.contains(viewport) {
            task?.cancel(); pending = nil
            restore(map); status(nil); return
        }
        if let pending, pending.contains(viewport) { return }
        if let failed, failed.contains(viewport) {
            status("Map layers are unavailable. Move the map to try again.")
            return
        }
        failed = nil
        task?.cancel()
        let dx = (bounds[2] - bounds[0]) * 0.2, dy = (bounds[3] - bounds[1]) * 0.2
        let padded = [max(release.bounds[0], bounds[0] - dx), max(release.bounds[1], bounds[1] - dy),
                      min(release.bounds[2], bounds[2] + dx), min(release.bounds[3], bounds[3] + dy)]
        let requested = Viewport(bounds: padded, zoom: viewport.zoom, key: viewport.key)
        pending = requested
        status(nil)
        // Keep data from a different activity or release out of the current view.
        if cached?.viewport.key != requested.key {
            clear(map)
        }
        task = Task { [weak self, weak map] in
            var written: URL?
            do {
                try await Task.sleep(for: .milliseconds(250))
                let data = try await load(padded, Double(requested.zoom), release)
                try Task.checkCancellation()
                let url = try await Self.write(data)
                written = url
                try Task.checkCancellation()
                guard let self, let map else { try? FileManager.default.removeItem(at: url); return }
                self.cached = (requested, url)
                self.pending = nil
                self.files.append(url)
                self.restore(map)
                // One previous file can still be in use by the native source worker.
                while self.files.count > 2 { try? FileManager.default.removeItem(at: self.files.removeFirst()) }
                self.status(nil)
            } catch is CancellationError {
                if let written { try? FileManager.default.removeItem(at: written) }
            } catch {
                guard !Task.isCancelled, let self else { return }
                self.pending = nil
                self.failed = viewport
                self.status("Map layers are unavailable. Move the map to try again.")
            }
        }
    }

    private func clear(_ map: OBCNativeMapView) {
        guard let source = map.style?.source(withIdentifier: identifier) as? MLNShapeSource else { return }
        source.url = nil
        source.shape = MLNShapeCollection(shapes: [])
    }
    func restore(_ map: OBCNativeMapView) {
        guard let cached, let source = map.style?.source(withIdentifier: identifier) as? MLNShapeSource,
              source.url != cached.url else { return }
        source.url = cached.url
    }
    func stop() {
        task?.cancel(); task = nil
        for file in files { try? FileManager.default.removeItem(at: file) }
        files.removeAll(); cached = nil; pending = nil; failed = nil
    }
    private nonisolated static func write(_ data: Data) async throws -> URL {
        try await Task.detached(priority: .utility) {
            let url = FileManager.default.temporaryDirectory.appending(path: "obc-viewport-\(UUID().uuidString).geojson")
            try data.write(to: url, options: .atomic)
            return url
        }.value
    }
}
#endif
