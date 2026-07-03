import Foundation
import OBCDomain

/// Garmin TCX → `ImportedRoute`. Reads `<Trackpoint>` geometry (position +
/// `<AltitudeMeters>`) from any `<Track>` — a `<Course>`'s or, as a fallback,
/// an `<Activity>`'s, so a shared workout file still imports as a route —
/// plus the course `<Name>`, `<CoursePoint>` waypoints, and the `<Author>`
/// name. Time and sensor data are ignored — a planned route has none.
public struct TCXRouteDecoder: RouteFileDecoder {
    public var fileExtensions: Set<String> { ["tcx"] }

    public init() {}

    public func decode(_ data: Data) throws -> ImportedRoute {
        let parser = XMLParser(data: data)
        let collector = TCXCollector()
        parser.delegate = collector
        guard parser.parse() else {
            let line = parser.parserError.map { " (\($0.localizedDescription))" } ?? ""
            throw FormatError.malformed(reason: "not valid XML\(line)")
        }
        // A present-but-invalid coordinate (non-finite or out of WGS-84 range)
        // is a hard reject, not a silent skip: it would poison distance math
        // (NaN) and the waypoint sort.
        guard !collector.malformed else {
            throw FormatError.malformed(reason: "coordinate is not finite or out of range")
        }
        guard !collector.points.isEmpty else {
            throw FormatError.malformed(reason: "no trackpoints")
        }
        return ImportedRoute(
            name: collector.courseName,
            creator: collector.author,
            points: collector.points,
            waypoints: WaypointPlacement.place(collector.rawWaypoints, along: collector.points)
        )
    }
}

/// The `XMLParser` delegate that walks a TCX document once. Class-based because
/// `XMLParserDelegate` requires `NSObject`; used strictly synchronously inside
/// `decode(_:)`, never across a concurrency boundary.
final class TCXCollector: NSObject, XMLParserDelegate {
    private(set) var author: String?
    private(set) var courseName: String?
    private(set) var points: [RoutePoint] = []
    private(set) var rawWaypoints: [RawWaypoint] = []
    /// Set when a `<Trackpoint>`/`<CoursePoint>` carried a parseable but invalid
    /// position — the decoder rejects the whole file.
    private(set) var malformed = false

    /// Whose `<Position>`/`<Name>` we're inside — TCX nests both under
    /// `<Trackpoint>` and `<CoursePoint>`, so the open container disambiguates.
    private enum Container { case trackpoint, coursePoint }

    // Walk state.
    private var path: [String] = []
    private var text = ""
    private var container: Container?
    private var pendingLatitude: Double?
    private var pendingLongitude: Double?
    private var pendingElevation: Double?
    private var pendingName: String?
    private var pendingPointType: String?
    private var pendingNotes: String?

    func parser(
        _ parser: XMLParser, didStartElement element: String, namespaceURI: String?,
        qualifiedName: String?, attributes: [String: String] = [:]
    ) {
        path.append(element)
        text = ""
        switch element {
        case "Trackpoint", "CoursePoint":
            container = element == "Trackpoint" ? .trackpoint : .coursePoint
            pendingLatitude = nil
            pendingLongitude = nil
            pendingElevation = nil
            pendingName = nil
            pendingPointType = nil
            pendingNotes = nil
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
        case "LatitudeDegrees" where path.last == "Position":
            pendingLatitude = Double(value)
        case "LongitudeDegrees" where path.last == "Position":
            pendingLongitude = Double(value)
        case "AltitudeMeters" where container == .trackpoint:
            // A non-finite <AltitudeMeters> is dropped to nil (no elevation), not
            // stored — it would poison ascent math.
            pendingElevation = Double(value).flatMap { $0.isFinite ? $0 : nil }
        case "Name":
            switch path.last {
            // First course's name wins — a multi-course file imports as one route.
            case "Course": if courseName == nil, !value.isEmpty { courseName = value }
            case "CoursePoint": pendingName = value
            case "Author": author = value.isEmpty ? nil : value
            default: break
            }
        case "PointType" where container == .coursePoint:
            pendingPointType = value
        case "Notes" where container == .coursePoint:
            pendingNotes = value.isEmpty ? nil : value
        case "Trackpoint":
            if let coordinate = pendingCoordinate() {
                points.append(RoutePoint(coordinate: coordinate, elevationMeters: pendingElevation))
            }
            container = nil
        case "CoursePoint":
            if let coordinate = pendingCoordinate() {
                // A course point's Name is schema-capped at 10 chars, so files
                // often lean on PointType ("Left", "Water", "Summit") instead.
                let name = pendingName?.isEmpty == false ? pendingName!
                    : (pendingPointType?.isEmpty == false ? pendingPointType! : "Waypoint")
                rawWaypoints.append(RawWaypoint(name: name, note: pendingNotes, coordinate: coordinate))
            }
            container = nil
        default:
            break
        }
        text = ""
    }

    /// A missing/unparseable lat or lon → `nil`, skipping the point as before.
    /// Present but invalid (non-finite / out of range) → flags `malformed`,
    /// which rejects the whole file.
    private func pendingCoordinate() -> Coordinate? {
        guard let lat = pendingLatitude, let lon = pendingLongitude else { return nil }
        let coordinate = Coordinate(latitude: lat, longitude: lon)
        guard coordinate.isValidGeographic else {
            malformed = true
            return nil
        }
        return coordinate
    }
}
