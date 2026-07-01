import Foundation
import OBCDomain

/// GPX 1.0/1.1 → `ImportedRoute` (the first registered `RouteFileDecoder`; TCX
/// follows with B6's share-sheet work). Reads what a route needs and nothing
/// more: `<trkpt>` (or `<rtept>`) geometry with `<ele>`, file-level `<wpt>`
/// waypoints, the route name, and the `creator` attribute for the E1 banner.
///
/// GPX carries waypoints as free-standing points, so each one is projected onto
/// the nearest track point to get its ride-order `index` + `distanceAlongMeters`
/// (what W1 renders). Time data is ignored — a planned route has none.
public struct GPXRouteDecoder: RouteFileDecoder {
    public var fileExtensions: Set<String> { ["gpx"] }

    public init() {}

    public func decode(_ data: Data) throws -> ImportedRoute {
        let parser = XMLParser(data: data)
        let collector = GPXCollector()
        parser.delegate = collector
        guard parser.parse() else {
            let line = parser.parserError.map { " (\($0.localizedDescription))" } ?? ""
            throw FormatError.malformed(reason: "not valid XML\(line)")
        }
        guard !collector.points.isEmpty else {
            throw FormatError.malformed(reason: "no track or route points")
        }
        return ImportedRoute(
            name: collector.routeName,
            creator: collector.creator,
            points: collector.points,
            waypoints: Self.placeWaypoints(collector.rawWaypoints, along: collector.points)
        )
    }

    /// Order free-standing GPX waypoints along the track: nearest-track-point
    /// projection → cumulative distance, then sort + re-index in ride order.
    static func placeWaypoints(
        _ raw: [GPXCollector.RawWaypoint], along points: [RoutePoint]
    ) -> [Waypoint] {
        guard !raw.isEmpty, points.count > 1 else { return [] }

        var cumulative: [Double] = [0]
        cumulative.reserveCapacity(points.count)
        for i in 1..<points.count {
            cumulative.append(cumulative[i - 1] + points[i - 1].coordinate.distance(to: points[i].coordinate))
        }

        let placed = raw.map { waypoint -> (RawPlacement) in
            var best = (index: 0, distance: Double.infinity)
            for (i, point) in points.enumerated() {
                let d = waypoint.coordinate.distance(to: point.coordinate)
                if d < best.distance { best = (i, d) }
            }
            return RawPlacement(waypoint: waypoint, along: cumulative[best.index])
        }

        return placed
            .sorted { $0.along < $1.along }
            .enumerated()
            .map { index, placement in
                Waypoint(
                    index: index,
                    name: placement.waypoint.name,
                    note: placement.waypoint.note,
                    distanceAlongMeters: placement.along,
                    coordinate: placement.waypoint.coordinate
                )
            }
    }

    private struct RawPlacement {
        let waypoint: GPXCollector.RawWaypoint
        let along: Double
    }
}

/// The `XMLParser` delegate that walks a GPX document once. Class-based because
/// `XMLParserDelegate` requires `NSObject`; used strictly synchronously inside
/// `decode(_:)`, never across a concurrency boundary.
final class GPXCollector: NSObject, XMLParserDelegate {
    struct RawWaypoint {
        var name: String
        var note: String?
        let coordinate: Coordinate
    }

    private(set) var creator: String?
    private(set) var routeName: String?
    private(set) var points: [RoutePoint] = []
    private(set) var rawWaypoints: [RawWaypoint] = []

    // Walk state.
    private var path: [String] = []
    private var text = ""
    private var pendingCoordinate: Coordinate?
    private var pendingElevation: Double?
    private var pendingWaypointName: String?
    private var pendingWaypointNote: String?
    /// `<trkpt>` wins over `<rtept>`; a file with only a `<rte>` still imports.
    private var routePointFallback: [RoutePoint] = []

    func parser(
        _ parser: XMLParser, didStartElement element: String, namespaceURI: String?,
        qualifiedName: String?, attributes: [String: String] = [:]
    ) {
        path.append(element)
        text = ""
        switch element {
        case "gpx":
            creator = attributes["creator"]
        case "trkpt", "rtept", "wpt":
            pendingCoordinate = Self.coordinate(from: attributes)
            pendingElevation = nil
            pendingWaypointName = nil
            pendingWaypointNote = nil
        default:
            break
        }
    }

    func parser(_ parser: XMLParser, foundCharacters string: String) {
        text += string
    }

    func parser(
        _ parser: XMLParser, didEndElement element: String,
        namespaceURI: String?, qualifiedName: String?
    ) {
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        path.removeLast()
        switch element {
        case "ele":
            pendingElevation = Double(value)
        case "name":
            switch path.last {
            case "wpt": pendingWaypointName = value
            // First name wins per scope; metadata's beats a later trk's only
            // when metadata comes first (it does — schema order).
            case "metadata", "trk", "rte": if routeName == nil, !value.isEmpty { routeName = value }
            default: break
            }
        case "desc" where path.last == "wpt":
            pendingWaypointNote = value.isEmpty ? nil : value
        case "trkpt":
            if let coordinate = pendingCoordinate {
                points.append(RoutePoint(coordinate: coordinate, elevationMeters: pendingElevation))
            }
        case "rtept":
            if let coordinate = pendingCoordinate {
                routePointFallback.append(RoutePoint(coordinate: coordinate, elevationMeters: pendingElevation))
            }
        case "wpt":
            if let coordinate = pendingCoordinate {
                rawWaypoints.append(RawWaypoint(
                    name: pendingWaypointName?.isEmpty == false ? pendingWaypointName! : "Waypoint",
                    note: pendingWaypointNote,
                    coordinate: coordinate
                ))
            }
        default:
            break
        }
        text = ""
    }

    func parserDidEndDocument(_ parser: XMLParser) {
        if points.isEmpty { points = routePointFallback }
    }

    private static func coordinate(from attributes: [String: String]) -> Coordinate? {
        guard
            let lat = attributes["lat"].flatMap(Double.init),
            let lon = attributes["lon"].flatMap(Double.init)
        else { return nil }
        return Coordinate(latitude: lat, longitude: lon)
    }
}
