import Foundation
import Observation
import OBCDomain
import OBCPlanner

/// The Route options panel: the route of the plan as A, then the alternatives of the route service, each named by
/// its trade-off. A preview never changes the plan; Use takes the option as one undo step.
@MainActor @Observable
final class PlannerRouteOptions {
    enum Status: Equatable { case idle, loading, ready, failed(String) }
    struct Item: Identifiable {
        /// The letter in the list and on the map.
        let id: String
        var name: String
        let option: RouteOption
        /// The plan before the last option taken. Use is undo.
        var isPrevious = false
    }

    private(set) var status = Status.idle
    /// Your route first.
    private(set) var items: [Item] = []
    /// The routing revision of `items`.
    private(set) var revision: Int?
    private(set) var previewed: String?
    /// The route that the last Use replaced, while undo brings it back in one step.
    private var previous: (item: Item, revision: Int, depth: Int)?
    @ObservationIgnored var nameTimeout = Duration.seconds(2)

    /// Where your route and the previewed option are farthest apart: the places of their letters on the map.
    private(set) var letterPlaces: (yours: Coordinate, preview: Coordinate)?

    var yours: Item? { items.first }
    var preview: Item? { items.dropFirst().first { $0.id == previewed } }
    func isCurrent(_ model: PlannerPreviewModel) -> Bool { revision == model.routingRevision }

    /// Your route cancels the preview.
    func select(_ id: String) {
        cancel()
        guard let yours, let preview = items.dropFirst().first(where: { $0.id == id }) else { return }
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

    /// Loads the options of the plan once for each routing revision. The corridor is named after the list shows.
    func load(_ model: PlannerPreviewModel) async {
        let revision = model.routingRevision
        guard model.offersRouteOptions, !(self.revision == revision && status == .ready) else { return }
        cancel(); self.revision = revision; status = .loading; items = []
        do {
            let options = try await model.routeOptions()
            guard revision == model.routingRevision else { return }
            var items = options.enumerated().map { index, option in
                Item(id: Self.letter(index), name: Self.name(option.reason), option: option)
            }
            if let previous, previous.revision == revision, previous.depth == model.undoDepth {
                items.append(Item(id: Self.letter(items.count), name: previous.item.name, option: previous.item.option, isPrevious: true))
            }
            self.items = items; status = .ready
            if let index = items.firstIndex(where: { $0.option.reason == .corridor }), let via = items[index].option.via,
               let name = await settlement(near: via, model: model), self.revision == revision, status == .ready {
                self.items[index].name = "Via \(name)"
            }
        } catch is CancellationError {
        } catch {
            guard revision == model.routingRevision else { return }
            status = .failed(error.localizedDescription)
        }
    }

    /// Takes the previewed option. The route it replaces stays listed until the plan changes again.
    func use(_ model: PlannerPreviewModel) {
        guard let item = preview, let revision, let yours else { return }
        if item.isPrevious {
            guard previous?.revision == model.routingRevision, previous?.depth == model.undoDepth else { return }
            previous = nil; cancel()
            model.undo()
            return
        }
        let before = PlannerPreviewPreset(yours.option.preference), after = PlannerPreviewPreset(item.option.preference)
        guard model.useRouteOption(item.option, revision: revision) else { return }
        cancel()
        previous = (Item(id: "", name: before == after ? "Previous route" : before.title, option: yours.option),
                    model.routingRevision, model.undoDepth)
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

    /// "B, Shorter, 25.2 kilometres, 669 metres climbing, 5.5 km shorter".
    func accessibilityLabel(_ item: Item, locale: Locale = .current) -> String {
        let path = item.option.path
        let detail = item.id == yours?.id ? PlannerPreviewPreset(item.option.preference).title
            : yours.map { Self.difference(path, from: $0.option.path, locale: locale).replacingOccurrences(of: " · ", with: ", ") }
        return [item.id, item.name, "\(OBCFormat.distanceValue(meters: path.distance, locale: locale)) kilometres",
                "\(OBCFormat.climbValue(meters: path.ascent, locale: locale)) metres climbing", detail,
                Self.unknownSurface(item.option.unknownSurfaceMeters, locale: locale)].compactMap { $0 }.joined(separator: ", ")
    }
}
