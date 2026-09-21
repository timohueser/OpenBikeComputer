import Foundation
import OBCDomain

/// GPX 1.0 and 1.1 to `ImportedRoute`. Reads `<trkpt>` (or `<rtept>`) geometry with
/// `<ele>`, file-level `<wpt>` waypoints with their `<sym>`/`<type>` symbol, the route
/// name, and the `creator` attribute. Time data is ignored: a planned route has none.
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
        // A present but invalid coordinate is a hard reject, not a silent skip: it
        // would poison the distance math and the waypoint sort with NaN. Checked
        // before the empty guard so a file of only bad points throws the real reason.
        guard !collector.malformed else {
            throw FormatError.malformed(reason: "coordinate is not finite or out of range")
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
    /// Set when a point carried a parseable but invalid coordinate; the decoder then
    /// rejects the whole file.
    private(set) var malformed = false

    // Walk state.
    private var path: [String] = []
    private var text = ""
    private var pendingCoordinate: Coordinate?
    private var pendingElevation: Double?
    private var pendingWaypointName: String?
    private var pendingWaypointNote: String?
    private var pendingWaypointSym: String?
    private var pendingWaypointType: String?
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
            pendingCoordinate = coordinate(from: attributes)
            pendingElevation = nil
            pendingWaypointName = nil
            pendingWaypointNote = nil
            pendingWaypointSym = nil
            pendingWaypointType = nil
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
            // A non-finite <ele> is dropped to nil; it would poison ascent math.
            pendingElevation = Double(value).flatMap { $0.isFinite ? $0 : nil }
        case "name":
            switch path.last {
            case "wpt": pendingWaypointName = value
            // The first name wins per scope; metadata comes before trk in schema order.
            case "metadata", "trk", "rte": if routeName == nil, !value.isEmpty { routeName = value }
            default: break
            }
        case "desc" where path.last == "wpt":
            pendingWaypointNote = value.isEmpty ? nil : value
        // The waypoint's icon: Garmin writes `<sym>`, RideWithGPS and Komoot `<type>`.
        // Both are kept; `WaypointSymbol` decides which wins and what it means.
        case "sym" where path.last == "wpt":
            pendingWaypointSym = value
        case "type" where path.last == "wpt":
            pendingWaypointType = value
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
                    coordinate: coordinate,
                    symbol: WaypointSymbol.symbol(sym: pendingWaypointSym, type: pendingWaypointType)
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

    /// An absent or unparseable `lat`/`lon` gives `nil` and the point is skipped.
    /// Present but invalid (`lat="inf"`, `lat="999"`) flags `malformed`, which rejects
    /// the whole file.
    private func coordinate(from attributes: [String: String]) -> Coordinate? {
        guard
            let lat = attributes["lat"].flatMap(Double.init),
            let lon = attributes["lon"].flatMap(Double.init)
        else { return nil }
        let coordinate = Coordinate(latitude: lat, longitude: lon)
        guard coordinate.isValidGeographic else {
            malformed = true
            return nil
        }
        return coordinate
    }
}
