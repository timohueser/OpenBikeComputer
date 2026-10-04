import Foundation
import OBCPlanner

enum PlannerPreviewQueryField: String, Identifiable {
    case what, name, area, radius
    var id: String { rawValue }
    var title: String {
        switch self { case .what: "Place types"; case .name: "Place name"; case .area: "Search area"; case .radius: "Search radius" }
    }
}

struct PlannerPreviewPlaceQuery: Equatable {
    enum Area: String, CaseIterable, Identifiable {
        case view, route, start, middle, end, section
        var id: String { rawValue }
        var title: String {
            switch self {
            case .view: "In this map view"
            case .route: "Along the route"
            case .start: "Near route start"
            case .middle: "Near route middle"
            case .end: "Near route end"
            case .section: "Route section"
            }
        }
    }
    var kinds: Set<PlannerPreviewPlace.Kind> = []
    var name = ""
    var area: Area = .view
    var radiusMeters: Double?
    var fromMeters = 0.0
    var toMeters: Double?

    static let placeTypes: [PlannerPreviewPlace.Kind] = [.cafe, .water, .shop, .camping]

    static func parse(_ text: String, hasRoute: Bool) -> Self? {
        let query = text.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return nil }
        let words: [(PlannerPreviewPlace.Kind, [String])] = [
            (.cafe, ["cafe", "coffee"]), (.water, ["water", "fountain"]),
            (.shop, ["shop", "supermarket", "resupply", "groceries", "bakery"]),
            (.camping, ["camp", "sleep"]),
        ]
        var value = Self(area: hasRoute && !query.contains("map") ? .route : .view)
        value.kinds = Set(words.filter { $0.1.contains(where: query.contains) }.map(\.0))
        if value.kinds.isEmpty { value.name = text.trimmingCharacters(in: .whitespacesAndNewlines) }
        if let match = quantities(in: query, pattern: #"(?:within|radius(?: of)?)\s+(\d+(?:\.\d+)?)\s*(km|m)\b"#).first {
            value.radiusMeters = match.value
        }
        if hasRoute {
            if query.contains("route start") { value.area = .start }
            if query.contains("route middle") { value.area = .middle }
            if query.contains("route end") { value.area = .end }
            if let range = query.range(of: #"(?:between|from)\s+(\d+(?:\.\d+)?)\s*(?:km)?\s*(?:and|to|–|-)\s+(\d+(?:\.\d+)?)\s*km\b"#, options: .regularExpression) {
                let numbers = String(query[range]).components(separatedBy: CharacterSet(charactersIn: "0123456789.").inverted).compactMap(Double.init)
                if numbers.count == 2 {
                    value.area = .section
                    value.fromMeters = min(numbers[0], numbers[1]) * 1_000
                    value.toMeters = max(numbers[0], numbers[1]) * 1_000
                }
            } else if let after = quantities(in: query, pattern: #"after\s+(\d+(?:\.\d+)?)\s*(km|m)\b"#).first {
                value.area = .section; value.fromMeters = after.value
            } else if let before = quantities(in: query, pattern: #"before\s+(\d+(?:\.\d+)?)\s*(km|m)\b"#).first {
                value.area = .section; value.toMeters = before.value
            }
        }
        if value.area == .view { value.radiusMeters = nil }
        return value
    }

    var fields: [PlannerPreviewQueryField] { [kinds.isEmpty ? .name : .what, .area] + (radiusMeters == nil ? [] : [.radius]) }
    var kindLabel: String { Self.placeTypes.filter { kinds.contains($0) }.map(\.title).joined(separator: ", ") }
    var areaLabel: String {
        area == .section ? "\(Self.kilometers(fromMeters))–\(toMeters.map(Self.kilometers) ?? "end") km" : area.title
    }
    func label(for field: PlannerPreviewQueryField) -> String {
        switch field {
        case .what: kindLabel
        case .name: name
        case .area: areaLabel
        case .radius: "Radius: \(Self.kilometers(radiusMeters ?? 0)) km"
        }
    }

    @MainActor
    func result(in model: PlannerPreviewModel, isInMapView: (PlannerPreviewPlace) -> Bool) -> PlannerPreviewQueryResult {
        let line = model.routeLine
        let candidates = model.mapPlaces.map { place in
            guard model.hasRoute else { return place }
            let projection = line.projection(of: place.coordinate, near: line.length / 2, window: line.length)
            return PlannerPreviewPlace(id: place.id, name: place.name, coordinate: place.coordinate, kind: place.kind,
                                       alongRouteMeters: projection.distance, offRouteMeters: projection.error,
                                       hours: place.hours, note: place.note, website: place.website, phone: place.phone, description: place.description, detailsLoaded: place.detailsLoaded)
        }
        let places = filter(candidates, routeLengthMeters: line.length, isInMapView: isInMapView)
        return .init(title: kinds.isEmpty ? name : kindLabel,
                     explanation: "", places: places, action: nil)
    }

    @MainActor
    func serverQuery(text: String, view: [Double]?, model: PlannerPreviewModel) -> PlannerSearchQuery {
        var query = PlannerSearchQuery(text: name.isEmpty ? text : name, view: view)
        query.kinds = kinds.map { $0 == .camping ? "campsite" : $0 == .shop ? "resupply" : $0.rawValue }.sorted()
        query.route = model.geometry
        query.routeLengthMeters = model.routeLine.length
        query.radiusMeters = radiusMeters
        if area != .view {
            query.alongRoute = true
            let length = model.routeLine.length
            switch area {
            case .start: query.toMeters = length / 3
            case .middle: query.fromMeters = length / 3; query.toMeters = length * 2 / 3
            case .end: query.fromMeters = length * 2 / 3
            case .section: query.fromMeters = fromMeters; query.toMeters = toMeters
            case .view, .route: break
            }
        }
        return query
    }

    func filter(_ places: [PlannerPreviewPlace], routeLengthMeters: Double,
                isInMapView: (PlannerPreviewPlace) -> Bool, matchesName: Bool = true) -> [PlannerPreviewPlace] {
        let range: ClosedRange<Double> = switch area {
        case .start: 0...(routeLengthMeters / 3)
        case .middle: (routeLengthMeters / 3)...(routeLengthMeters * 2 / 3)
        case .end: (routeLengthMeters * 2 / 3)...routeLengthMeters
        case .section: min(fromMeters, toMeters ?? routeLengthMeters)...max(fromMeters, toMeters ?? routeLengthMeters)
        case .view, .route: 0...max(0, routeLengthMeters)
        }
        return places.filter { place in
            guard kinds.isEmpty ? (!matchesName || place.name.localizedStandardContains(name)) : kinds.contains(place.kind) else { return false }
            if area == .view { return isInMapView(place) }
            guard range.contains(place.alongRouteMeters) else { return false }
            return radiusMeters.map { place.offRouteMeters <= $0 } ?? true
        }.sorted { $0.alongRouteMeters < $1.alongRouteMeters }
    }

    private static func kilometers(_ meters: Double) -> String {
        (meters / 1_000).formatted(.number.precision(.fractionLength(0...1)))
    }
    private static func quantities(in text: String, pattern: String) -> [(value: Double, unit: String)] {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [] }
        let source = text as NSString
        return regex.matches(in: text, range: NSRange(location: 0, length: source.length)).compactMap { match in
            guard let number = Double(source.substring(with: match.range(at: 1))) else { return nil }
            let unit = source.substring(with: match.range(at: 2))
            return (number * (unit == "km" ? 1_000 : 1), unit)
        }
    }
}
