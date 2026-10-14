import Foundation
import Observation
import OBCDomain
import OBCPlanner

/// The Route options panel: the plan's own route as A, then the other routes of the route service, each named by its
/// trade-off. A preview never changes the plan; Use takes a route as one undo step.
@MainActor @Observable
final class PlannerRouteOptions {
    enum Status: Equatable { case idle, loading, ready, failed(String) }
    struct Item: Identifiable {
        /// The letter in the list and on the map.
        let id: String
        var name: String
        let option: RouteOption
    }

    private(set) var status = Status.idle
    /// The other routes, B first.
    private(set) var items: [Item] = []
    /// The routing revision of `items`.
    private(set) var revision: Int?
    private(set) var previewed: String?
    /// The route that the last Use made your route; nil while your route is the plan's own.
    private var chosen: Item?
    private var unknownSurfaceMeters = 0.0
    @ObservationIgnored var nameTimeout = Duration.seconds(2)

    /// Where your route and the previewed route are farthest apart: the places of their letters on the map.
    private(set) var letterPlaces: (yours: Coordinate, preview: Coordinate)?

    var preview: Item? { items.first { $0.id == previewed } }
    func isCurrent(_ model: PlannerPreviewModel) -> Bool { revision == model.routingRevision }

    /// The plan's route with its preset, and the via of the route that Use took.
    func yours(_ model: PlannerPreviewModel) -> Item? {
        model.routePath.map {
            Item(id: "A", name: "Your route", option: RouteOption(reason: chosen?.option.reason ?? .primary,
                preference: model.preset.preference, via: chosen?.option.via, path: $0, unknownSurfaceMeters: unknownSurfaceMeters))
        }
    }

    /// Your route cancels the preview.
    func select(_ id: String, model: PlannerPreviewModel) {
        cancel()
        guard let yours = yours(model), let preview = items.first(where: { $0.id == id }) else { return }
        previewed = id
        let a = MeasuredLine(routePoints: yours.option.path.points), b = MeasuredLine(routePoints: preview.option.path.points)
        letterPlaces = (Self.farthest(a, from: b), Self.farthest(b, from: a))
    }
    func cancel() { previewed = nil; letterPlaces = nil }

    private static func farthest(_ line: MeasuredLine, from other: MeasuredLine) -> Coordinate {
        (1..<32).map { line.coordinate(at: line.length * Double($0) / 32) }
            .map { ($0, other.projection(of: $0, near: other.length / 2, window: other.length).error) }
            .max { $0.1 < $1.1 }?.0 ?? line.coordinate(at: 0)
    }

    /// Loads the routes once for each routing revision. The corridor is named after the list shows.
    func load(_ model: PlannerPreviewModel) async {
        let revision = model.routingRevision
        guard model.offersRouteOptions, !(self.revision == revision && status == .ready) else { return }
        cancel(); self.revision = revision; status = .loading; items = []; chosen = nil
        do {
            let options = try await model.routeOptions()
            guard revision == model.routingRevision else { return }
            unknownSurfaceMeters = options.first?.unknownSurfaceMeters ?? 0
            let items = options.dropFirst().enumerated().map { index, option in
                Item(id: Self.letter(index + 1), name: Self.name(option.reason), option: option)
            }
            self.items = items; status = .ready
            if let corridor = items.first(where: { $0.option.via != nil }), let via = corridor.option.via,
               let name = await settlement(near: via, model: model), self.revision == revision, status == .ready,
               let index = self.items.firstIndex(where: { $0.id == corridor.id }) {
                self.items[index].name = "Via \(name)"
            }
        } catch is CancellationError {
        } catch {
            guard revision == model.routingRevision else { return }
            status = .failed(error.localizedDescription)
        }
    }

    /// Takes the previewed route. The list stays: your route takes its place under its own name.
    func use(_ model: PlannerPreviewModel) {
        guard let item = preview, let revision, let yours = yours(model),
              model.useRouteOption(item.option, revision: revision, replacingVia: chosen?.option.via != nil) else { return }
        let previous = Item(id: "", name: chosen?.name ?? PlannerPreviewPreset(yours.option.preference).title, option: yours.option)
        cancel()
        chosen = item
        unknownSurfaceMeters = item.option.unknownSurfaceMeters
        items = (items.filter { $0.id != item.id } + [previous]).enumerated().map {
            Item(id: Self.letter($0.offset + 1), name: $0.element.name, option: $0.element.option)
        }
        self.revision = model.routingRevision
    }

    /// The nearest city, town or village within 3 km of `via`, from the planner search. Nil on a failure or after
    /// `nameTimeout`.
    private func settlement(near via: Coordinate, model: PlannerPreviewModel) async -> String? {
        guard let release = model.release else { return nil }
        return await Self.settlement(near: via, service: model.service, release: release, timeout: nameTimeout)
    }

    private nonisolated static func settlement(near via: Coordinate, service: any PlannerDataSource, release: PlannerRelease,
                                               timeout: Duration) async -> String? {
        var query = PlannerSearchQuery(text: "places", view: [via.longitude - 0.05, via.latitude - 0.05,
                                                              via.longitude + 0.05, via.latitude + 0.05])
        query.kinds = ["town"]; query.radiusMeters = 3_000
        return await withTaskGroup(of: String?.self) { [query] group in
            group.addTask {
                let places = (try? await service.search(query, release: release)) ?? []
                return places.filter { ["city", "town", "village"].contains($0.kind) }
                    .min { $0.coordinate.distance(to: via) < $1.coordinate.distance(to: via) }?.name
            }
            group.addTask { try? await Task.sleep(for: timeout); return nil }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
    }

    static func letter(_ index: Int) -> String { String(UnicodeScalar(UInt8(65 + min(index, 25)))) }

    static func name(_ reason: RouteOption.Reason) -> String {
        switch reason {
        case .primary: "Your route"
        case .shorter: "Shorter"
        case .lessClimbing: "Less climbing"
        case .smoother: "Smoother"
        case .corridor: "Other way"
        }
    }

    /// "5.5 km shorter · 237 m less climbing". The climb stays out when a height of either route is unknown.
    static func difference(_ path: PlannedPath, from base: PlannedPath, locale: Locale = .current) -> String {
        let km = OBCFormat.distanceValue(meters: abs(path.distance - base.distance), locale: locale)
        let distance = km == OBCFormat.distanceValue(meters: 0, locale: locale) ? "Same distance"
            : "\(km) km \(path.distance < base.distance ? "shorter" : "longer")"
        guard ![path, base].contains(where: { $0.points.contains { $0.elevationMeters == nil } }) else { return distance }
        let metres = Int((path.ascent - base.ascent).rounded())
        let climb = metres == 0 ? "same climbing"
            : "\(OBCFormat.climbValue(meters: Double(abs(metres)), locale: locale)) m \(metres < 0 ? "less" : "more") climbing"
        return "\(distance) · \(climb)"
    }

    /// "0.2 km unknown surface"; nil when there is none.
    static func unknownSurface(_ meters: Double, locale: Locale = .current) -> String? {
        meters >= 5 ? "\(OBCFormat.shortDistance(meters: meters, locale: locale)) unknown surface" : nil
    }

    /// "B, Shorter, 25.2 kilometres, 669 metres climbing, 5.5 km shorter". Your route names its preset instead.
    static func accessibilityLabel(_ item: Item, yours: Item, locale: Locale = .current) -> String {
        let path = item.option.path
        let detail = item.id == yours.id ? PlannerPreviewPreset(item.option.preference).title
            : difference(path, from: yours.option.path, locale: locale).replacingOccurrences(of: " · ", with: ", ")
        return [item.id, item.name, "\(OBCFormat.distanceValue(meters: path.distance, locale: locale)) kilometres",
                "\(OBCFormat.climbValue(meters: path.ascent, locale: locale)) metres climbing", detail,
                unknownSurface(item.option.unknownSurfaceMeters, locale: locale)].compactMap { $0 }.joined(separator: ", ")
    }
}
