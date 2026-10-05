import Foundation
import OBCDomain

/// Each route uses one complete graph. Separately downloaded graphs never form a union.
public actor LocalFirstPlanner: PlannerDataSource {
    public nonisolated var supportsOffline: Bool { true }
    public typealias Open = @Sendable (OfflineMap, URL) async throws -> any PlannerDataSource
    private let store: OfflineMapStore
    private let online: any PlannerDataSource
    private let open: Open
    private var cached: (id: String, source: any PlannerDataSource)?

    public init(store: OfflineMapStore, online: any PlannerDataSource = PlannerService.shared, open: @escaping Open) {
        self.store = store; self.online = online; self.open = open
    }

    private func installed() async throws -> [OfflineMap] {
        let maps: [OfflineMap]
        do { maps = try await store.maps() }
        catch { try cancellation(error); maps = [] }
        if let cached, !maps.contains(where: { $0.id == cached.id }) { self.cached = nil }
        return maps.sorted { area($0.bounds) < area($1.bounds) }
    }

    private func source(_ map: OfflineMap) async throws -> any PlannerDataSource {
        if let cached, cached.id == map.id { return cached.source }
        let source = try await open(map, store.root.appending(path: "releases/\(map.id)"))
        cached = (map.id, source)
        return source
    }

    public func release() async throws -> PlannerRelease { try await mapRelease(bounds: nil) }

    public func mapRelease(bounds: [Double]?, allowNetwork: Bool = true) async throws -> PlannerRelease {
        let maps = try await installed()
        for map in maps where bounds == nil || map.contains(bounds!) {
            do { return try await source(map).release() }
            catch { try cancellation(error) }
        }
        do {
            guard allowNetwork else { throw PlannerFailure.offlineUnavailable }
            return try await online.mapRelease(bounds: bounds)
        }
        catch {
            try cancellation(error)
            // Keep the available part visible when the viewport extends beyond a local map.
            for map in maps where bounds == nil || intersects(map.bounds, bounds!) {
                do { return try await source(map).release() }
                catch { try cancellation(error) }
            }
            throw PlannerFailure.offlineUnavailable
        }
    }

    public func route(points: [Coordinate], turnarounds: [Int] = [], activity: RouteActivity, preference: RoutePreference,
                      release: PlannerRelease) async throws -> PlannedPath {
        guard (2...64).contains(points.count) else { throw PlannerFailure.invalidData }
        for map in try await installed() where points.allSatisfy({ covers(map.bounds, $0) }) {
            do {
                let source = try await source(map), local = try await source.release()
                return try await source.route(points: points, turnarounds: turnarounds, activity: activity, preference: preference, release: local)
            } catch { try cancellation(error) }
        }
        do {
            let remote = try await remoteRelease(release)
            return try await online.route(points: points, turnarounds: turnarounds, activity: activity, preference: preference, release: remote)
        } catch { throw try fallbackError(error) }
    }

    public func shape(line: [Coordinate], profile: String) async throws -> PlannedShape {
        for map in try await installed() where line.allSatisfy({ covers(map.bounds, $0) }) {
            do { return try await source(map).shape(line: line, profile: profile) }
            catch { try cancellation(error) }
        }
        do { return try await online.shape(line: line, profile: profile) }
        catch { throw try fallbackError(error) }
    }

    public func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        for map in try await installed() where coversSearch(map, query) {
            do {
                let source = try await source(map), local = try await source.release()
                var bounded = query
                if query.source != nil, let view = query.view {
                    bounded.view = [max(view[0], map.bounds[0]), max(view[1], map.bounds[1]),
                                    min(view[2], map.bounds[2]), min(view[3], map.bounds[3])]
                }
                return try await source.search(bounded, release: local)
            } catch { try cancellation(error) }
        }
        do { return try await online.search(query, release: remoteRelease(release)) }
        catch { throw try fallbackError(error) }
    }

    public func profiles(release: PlannerRelease) async throws -> [String]? {
        if release.isLocal, let map = try await installed().first(where: { $0.id == release.id }) {
            let source = try await source(map)
            return try await source.profiles(release: source.release())
        }
        return try await online.profiles(release: remoteRelease(release))
    }

    private func remoteRelease(_ release: PlannerRelease) async throws -> PlannerRelease {
        if !release.isLocal { return release }
        return try await online.release()
    }

    private func cancellation(_ error: Error) throws {
        if error is CancellationError || (error as? URLError)?.code == .cancelled { throw CancellationError() }
        try Task.checkCancellation()
    }

    private func fallbackError(_ error: Error) throws -> Error {
        try cancellation(error)
        return (error as? PlannerFailure) == .unavailable ? PlannerFailure.offlineUnavailable : error
    }

    private func area(_ box: [Double]) -> Double { (box[2] - box[0]) * (box[3] - box[1]) }
    private func coversSearch(_ map: OfflineMap, _ query: PlannerSearchQuery) -> Bool {
        let view = query.view ?? map.bounds
        if query.source != nil {
            guard OfflineMap.valid(view) else { return false }
            // Exact-source requests use the viewport centre as the selected POI's anchor.
            return covers(map.bounds, Coordinate(latitude: (view[1] + view[3]) / 2,
                                                 longitude: (view[0] + view[2]) / 2))
        }
        guard map.contains(view), query.route.allSatisfy({ covers(map.bounds, $0) }) else { return false }
        guard !query.kinds.isEmpty else { return true }
        let box: [Double], radius: Double
        if query.alongRoute && !query.route.isEmpty {
            // Cover the whole corridor conservatively, including section searches.
            let xs = query.route.map(\.longitude), ys = query.route.map(\.latitude)
            box = [xs.min()!, ys.min()!, xs.max()!, ys.max()!]
            radius = query.radiusMeters ?? (query.toMeters == query.fromMeters ? 3_000 : 1_000)
        } else if let meters = query.radiusMeters {
            let x = (view[0] + view[2]) / 2, y = (view[1] + view[3]) / 2
            box = [x, y, x, y]; radius = meters
        } else { return true }
        guard radius.isFinite, radius >= 0 else { return false }
        // Match the search resolver's latitude scale; the outer latitude bounds every segment.
        let dy = radius / 111_200
        let dx = dy / cos(max(abs(box[1]), abs(box[3])) * .pi / 180)
        return map.contains([box[0] - dx, box[1] - dy, box[2] + dx, box[3] + dy])
    }
    private func covers(_ box: [Double], _ point: Coordinate) -> Bool {
        point.longitude >= box[0] && point.longitude <= box[2] && point.latitude >= box[1] && point.latitude <= box[3]
    }
    private func intersects(_ a: [Double], _ b: [Double]) -> Bool {
        OfflineMap.valid(b) && a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
    }
}
