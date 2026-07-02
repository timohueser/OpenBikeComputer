import Foundation
import OBCDomain

/// GPX 1.0/1.1 → `ImportedRoute`. Reads what a route needs and nothing
/// more: `<trkpt>` (or `<rtept>`) geometry with `<ele>`, file-level `<wpt>`
/// waypoints, the route name, and the `creator` attribute for the E1 banner.
/// Time data is ignored — a planned route has none.
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
            waypoints: WaypointPlacement.place(collector.rawWaypoints, along: collector.points)
        )
    }
}

/// The `XMLParser` delegate that walks a GPX document once. Class-based because
/// `XMLParserDelegate` requires `NSObject`; used strictly synchronously inside
/// `decode(_:)`, never across a concurrency boundary.
final class GPXCollector: NSObject, XMLParserDelegate {
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
