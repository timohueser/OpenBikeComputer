import Foundation
import Observation
import OBCDomain
import OBCPlanner

/// A start from a place has its name; a map point has the name of the nearest place.
struct PlannerRouteStart: Equatable {
    var coordinate: Coordinate
    var name: String
}

struct PlannerRouteFilters: Equatable {
    var radiusKm = 10.0
    var shape = RouteShape.loop
    var distanceKm = RouteBounds()
    var climbM = RouteBounds()
    /// Grade indices of the hardest part: T1–T4 for hiking, S0–S3 for mountain bike. Other activities have no difficulty filter.
    var hardest = 0...2
    var sort = RouteSort.nearest
}

/// The Routes view state. It outlives the view, so "Find another route" finds the filters and the list again.
@MainActor @Observable
final class PlannerRouteFinder {
    enum Status { case idle, loading, ready, failed }
    /// The shown route, its distance from the start when it is a match, and the stages of its long route once they load.
    struct Detail: Equatable {
        let route: CatalogRecord
        var distanceM: Double?
        var family: CatalogRecord?
        var stages: [CatalogRecord]?
        var failed = false
    }
    enum PlanState: Equatable { case loading, ready(RoutePlan), tooLong, failed, invalid }

    var start: PlannerRouteStart?
    var filters = PlannerRouteFilters()
    private(set) var matches: [RouteMatch] = []
    private(set) var status = Status.idle
    private(set) var progress = (loaded: 0, total: 0)
    private(set) var hint: RouteHint?
    /// The listed rows; the map draws their lines.
    var shown = 20
    private(set) var detail: Detail?
    /// The routed plan of the detail, for its profile, figures and time.
    private(set) var preview: (plan: RoutePlan, path: PlannedPath, profile: PlannerPreviewGrade)?
    private var catalog: (release: String, value: RouteCatalog)?
    private var cells: [String: Task<[CatalogRecord]?, any Error>] = [:]
    private var records: [Int: CatalogRecord] = [:]
    @ObservationIgnored private var lines: [Int: [Coordinate]] = [:]
    private var serial = 0
    @ObservationIgnored private let makeCatalog: (PlannerRelease) -> RouteCatalog?

    init(catalog: @escaping (PlannerRelease) -> RouteCatalog? = { RouteCatalog(release: $0) }) { makeCatalog = catalog }

    /// Whether the release has a route catalog.
    var available: Bool { catalog != nil }
    /// Offline, only routes wholly inside the download match.
    var offline: Bool { catalog?.value.covered != nil }

    func use(_ release: PlannerRelease?) {
        guard let release else { catalog = nil; return }
        guard catalog?.release != release.id else { return }
        catalog = makeCatalog(release).map { (release.id, $0) }
        cells = [:]; records = [:]; lines = [:]; matches = []; detail = nil; status = .idle
    }

    func query(_ filters: PlannerRouteFilters, activity: RouteActivity) -> RouteQuery? {
        guard let start else { return nil }
        return RouteQuery(start: start.coordinate, radiusKm: filters.radiusKm, activity: activity, shape: filters.shape,
                          distanceKm: filters.distanceKm, climbM: filters.climbM, hardest: filters.hardest, sort: filters.sort,
                          covered: catalog?.value.covered)
    }

    func search(activity: RouteActivity) async {
        guard let query = query(filters, activity: activity), catalog != nil else { return }
        serial += 1
        let id = serial
        status = .loading; progress = (0, 0)
        let loader: RouteCellLoader = { [weak self] cell in try await self?.cell(cell, counting: id) ?? nil }
        do {
            let found = try await SignedRoutes.search(query, loadCell: loader)
            let hint = found.isEmpty ? try await SignedRoutes.hint(query, loadCell: loader) : nil
            guard id == serial else { return }
            matches = found; self.hint = hint; shown = 20; status = .ready
        } catch {
            if id == serial && !(error is CancellationError) { status = .failed }
        }
    }

    /// The number of matches for draft filters, for the filter page.
    func count(_ filters: PlannerRouteFilters, activity: RouteActivity) async throws -> Int {
        guard let query = query(filters, activity: activity) else { return 0 }
        return try await SignedRoutes.search(query) { [weak self] cell in try await self?.cell(cell, counting: nil) ?? nil }.count
    }

    private func cell(_ id: String, counting search: Int?) async throws -> [CatalogRecord]? {
        guard let catalog = catalog?.value else { return nil }
        let counted = search != nil && search == serial
        if counted { progress.total += 1 }
        defer { if counted && search == serial { progress.loaded += 1 } }
        let task = cells[id] ?? Task { try await catalog.loadCell(id) }
        cells[id] = task
        do {
            let file = try await task.value
            file?.forEach { records[$0.id] = $0 }
            return file
        } catch {
            // A failed cell loads again on the next search.
            cells[id] = nil
            throw error
        }
    }

    func deselect() { preview = nil; detail = nil }

    /// Shows a record, and loads the stages of its long route.
    func select(_ route: CatalogRecord) async {
        preview = nil
        let selected = Detail(route: route, distanceM: matches.first { $0.route.id == route.id }?.distanceM)
        detail = selected
        let family = route.stages != nil ? route : route.parent.flatMap { records[$0] }
        guard let family, let ids = family.stages else { return }
        do {
            for cell in family.cells { _ = try await self.cell(cell, counting: nil) }
            let stages = ids.compactMap { records[$0] }
            guard detail == selected else { return }
            detail?.family = family
            if stages.count == ids.count { detail?.stages = stages } else { detail?.failed = true }
        } catch {
            if detail == selected { detail?.failed = true }
        }
    }

    var plan: PlanState {
        guard let detail else { return .invalid }
        guard detail.route.stages != nil else {
            guard let plan = detail.route.plan, plan.requestPoints(loop: detail.route.loop).count <= RoutePlan.maxPoints else { return .invalid }
            return .ready(plan)
        }
        if detail.failed { return .failed }
        guard let stages = detail.stages else { return .loading }
        let plans = stages.compactMap(\.plan)
        guard plans.count == stages.count else { return .invalid }
        return RoutePlan.joined(plans).map(PlanState.ready) ?? .tooLong
    }

    /// Routes the plan of the detail once, with the request that "Plan this route" makes.
    func routePreview(service: any PlannerDataSource, release: PlannerRelease, activity: RouteActivity) async {
        guard case .ready(let plan) = plan, let route = detail?.route, preview?.plan != plan else { return }
        let points = plan.requestPoints(loop: route.loop)
        let turnarounds = plan.turnarounds.filter { $0 > 0 && $0 < points.count - 1 }
        guard let path = try? await service.route(points: points, turnarounds: turnarounds, activity: activity, preference: .balanced,
                                                  release: release), self.plan == .ready(plan) else { return }
        preview = (plan, path, PlannerPreviewGrade(routePoints: path.points))
    }

    /// The line of a record: its own, or the lines of the stages of a long route in order.
    func line(_ route: CatalogRecord) -> [Coordinate] {
        if let line = lines[route.id] { return line }
        let line = route.stages.map { $0.flatMap { records[$0]?.line ?? [] } } ?? route.line
        if route.stages == nil || route.stages?.allSatisfy({ records[$0] != nil }) == true { lines[route.id] = line }
        return line
    }

    func record(_ id: Int) -> CatalogRecord? { records[id] }
}
